//! Counting 4xx responses per client address over a minute (decision J6), in a
//! fixed table: nothing is allocated per call and the table never grows. A
//! collision can only make the trigger fire early for an unrelated client.
//! The address is never a metric label; it appears only in a sample.

use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};

const SLOTS: usize = 4096;
const WINDOW_MS: u64 = 60_000;
/// Once a client is above the threshold, one more request in this many is
/// sampled, so a scanner cannot fill the table.
pub const SAMPLE_EVERY: u32 = 50;

/// What one more 4xx from a client means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientVerdict {
    /// Under the threshold this minute.
    Below,
    /// This request took the client to the threshold: sample it.
    Crossed,
    /// Above the threshold; sampled when `true` (one in [`SAMPLE_EVERY`]).
    Above { sampled: bool },
}

impl ClientVerdict {
    /// Whether this request should be sampled.
    pub fn is_sampled(self) -> bool {
        matches!(self, Self::Crossed | Self::Above { sampled: true })
    }
}

/// The table. Each slot packs the minute and the count in one atomic.
#[derive(Debug)]
pub struct ClientErrors {
    slots: Box<[AtomicU64]>,
}

impl Default for ClientErrors {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientErrors {
    pub fn new() -> Self {
        Self {
            slots: (0..SLOTS).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Counts one 4xx response to `client` at `now_ms` and says whether it
    /// should be sampled given `threshold` per minute.
    pub fn record(&self, client: IpAddr, now_ms: u64, threshold: u32) -> ClientVerdict {
        let slot = &self.slots[slot_of(client)];
        let minute = (now_ms / WINDOW_MS) as u32;
        let mut current = slot.load(Ordering::Relaxed);
        let count = loop {
            let (window, count) = ((current >> 32) as u32, current as u32);
            let next_count = if window == minute {
                count.saturating_add(1)
            } else {
                1
            };
            let next = (u64::from(minute) << 32) | u64::from(next_count);
            match slot.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
                Ok(_) => break next_count,
                Err(seen) => current = seen,
            }
        };
        match count.cmp(&threshold) {
            std::cmp::Ordering::Less => ClientVerdict::Below,
            std::cmp::Ordering::Equal => ClientVerdict::Crossed,
            std::cmp::Ordering::Greater => ClientVerdict::Above {
                sampled: (count - threshold).is_multiple_of(SAMPLE_EVERY),
            },
        }
    }
}

fn slot_of(client: IpAddr) -> usize {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    match client {
        IpAddr::V4(ip) => mix(&ip.octets()),
        IpAddr::V6(ip) => mix(&ip.octets()),
    }
    (hash % SLOTS as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([203, 0, 113, last])
    }

    #[test]
    fn the_request_that_reaches_the_threshold_is_the_one_sampled() {
        let table = ClientErrors::new();
        let verdicts: Vec<ClientVerdict> = (0..5).map(|_| table.record(ip(1), 0, 3)).collect();
        assert_eq!(verdicts[0], ClientVerdict::Below);
        assert_eq!(verdicts[1], ClientVerdict::Below);
        assert_eq!(verdicts[2], ClientVerdict::Crossed);
        assert_eq!(verdicts[3], ClientVerdict::Above { sampled: false });
        assert!(verdicts[2].is_sampled());
        assert!(!verdicts[0].is_sampled());
    }

    #[test]
    fn above_the_threshold_one_in_n_is_sampled() {
        let table = ClientErrors::new();
        let mut sampled = 0;
        for _ in 0..(2 * SAMPLE_EVERY + 1) {
            if table.record(ip(2), 0, 1).is_sampled() {
                sampled += 1;
            }
        }
        // The crossing itself, then one more per SAMPLE_EVERY.
        assert_eq!(sampled, 1 + 2);
    }

    #[test]
    fn the_count_starts_over_each_minute() {
        let table = ClientErrors::new();
        for _ in 0..5 {
            table.record(ip(3), 1_000, 3);
        }
        assert_eq!(
            table.record(ip(3), 61_000, 3),
            ClientVerdict::Below,
            "a new minute"
        );
        assert_eq!(table.record(ip(3), 61_500, 3), ClientVerdict::Below);
        assert_eq!(table.record(ip(3), 62_000, 3), ClientVerdict::Crossed);
    }

    #[test]
    fn clients_are_counted_apart() {
        let table = ClientErrors::new();
        for _ in 0..10 {
            table.record(ip(4), 0, 3);
        }
        assert_eq!(table.record(ip(5), 0, 3), ClientVerdict::Below);
    }

    #[test]
    fn ipv6_addresses_are_counted_too() {
        let table = ClientErrors::new();
        let v6: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(table.record(v6, 0, 2), ClientVerdict::Below);
        assert_eq!(table.record(v6, 0, 2), ClientVerdict::Crossed);
    }

    #[test]
    fn a_collision_only_makes_the_trigger_early_and_never_panics() {
        let table = ClientErrors::new();
        let mut by_slot = std::collections::HashMap::new();
        let mut pair = None;
        for last in 0..=255u8 {
            for third in 0..=40u8 {
                let candidate = IpAddr::from([10, 0, third, last]);
                if let Some(other) = by_slot.insert(slot_of(candidate), candidate) {
                    pair = Some((other, candidate));
                }
            }
        }
        let (a, b) = pair.expect("10,000 addresses over 4096 slots must collide");
        table.record(a, 0, 3);
        table.record(a, 0, 3);
        assert_eq!(
            table.record(b, 0, 3),
            ClientVerdict::Crossed,
            "b inherits a's count"
        );
    }

    #[test]
    fn the_table_has_a_fixed_size() {
        assert_eq!(ClientErrors::new().slots.len(), SLOTS);
    }
}
