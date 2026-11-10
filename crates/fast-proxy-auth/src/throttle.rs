use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};

use thiserror::Error;

/// The source address is throttled; answer 429 with this `Retry-After`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("too many failed authentications; retry in {retry_after_secs}s")]
pub struct Throttled {
    pub retry_after_secs: u64,
}

struct Entry {
    /// Failure times in milliseconds, oldest first, at most `max_failures`.
    failures: VecDeque<i64>,
    /// Recency stamp, the key of this entry in `State::by_recency`.
    stamp: u64,
}

#[derive(Default)]
struct State {
    entries: HashMap<IpAddr, Entry>,
    by_recency: BTreeMap<u64, IpAddr>,
    next_stamp: u64,
}

/// Slows down guessing: after `max_failures` failed authentications from one
/// address within `window_ms`, that address is refused (even with a valid
/// token) until its oldest failure leaves the window. Counters live in this
/// instance only, and memory is bounded by evicting the least recently failing
/// address. The clock is a parameter.
pub struct FailureThrottle {
    max_failures: usize,
    window_ms: i64,
    capacity: usize,
    state: Mutex<State>,
}

impl FailureThrottle {
    /// `capacity` is how many distinct addresses are remembered.
    pub fn new(max_failures: usize, window_ms: i64, capacity: usize) -> Self {
        Self {
            max_failures: max_failures.max(1),
            window_ms,
            capacity: capacity.max(1),
            state: Mutex::default(),
        }
    }

    /// Whether `source` may attempt authentication at `now_ms`.
    pub fn check(&self, source: IpAddr, now_ms: i64) -> Result<(), Throttled> {
        let mut state = self.lock();
        let Some(entry) = state.entries.get_mut(&source) else {
            return Ok(());
        };
        drop_expired(&mut entry.failures, now_ms, self.window_ms);
        if entry.failures.len() < self.max_failures {
            if entry.failures.is_empty() {
                state.forget(source);
            }
            return Ok(());
        }
        let free_at_ms = entry.failures[0] + self.window_ms;
        let wait_ms = u64::try_from(free_at_ms - now_ms).unwrap_or(0);
        Err(Throttled {
            retry_after_secs: wait_ms.div_ceil(1000).max(1),
        })
    }

    /// Records a failed authentication from `source`. While the address is
    /// already throttled this changes nothing, so the block ends on schedule
    /// however hard it is pushed.
    pub fn record_failure(&self, source: IpAddr, now_ms: i64) {
        let mut state = self.lock();
        let stamp = state.take_stamp();
        let entry = state.entries.entry(source).or_insert_with(|| Entry {
            failures: VecDeque::new(),
            stamp,
        });
        drop_expired(&mut entry.failures, now_ms, self.window_ms);
        if entry.failures.len() >= self.max_failures {
            return;
        }
        entry.failures.push_back(now_ms);
        let old_stamp = std::mem::replace(&mut entry.stamp, stamp);
        state.by_recency.remove(&old_stamp);
        state.by_recency.insert(stamp, source);
        while state.entries.len() > self.capacity {
            state.evict_least_recent();
        }
    }

    /// How many addresses are currently remembered.
    pub fn tracked_addresses(&self) -> usize {
        self.lock().entries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl State {
    fn take_stamp(&mut self) -> u64 {
        self.next_stamp += 1;
        self.next_stamp
    }

    fn forget(&mut self, source: IpAddr) {
        if let Some(entry) = self.entries.remove(&source) {
            self.by_recency.remove(&entry.stamp);
        }
    }

    fn evict_least_recent(&mut self) {
        if let Some((_, source)) = self.by_recency.pop_first() {
            self.entries.remove(&source);
        }
    }
}

fn drop_expired(failures: &mut VecDeque<i64>, now_ms: i64, window_ms: i64) {
    while failures.front().is_some_and(|&at| at + window_ms <= now_ms) {
        failures.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: i64 = 60_000;

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([10, 0, 0, last])
    }

    fn throttle() -> FailureThrottle {
        FailureThrottle::new(10, WINDOW, 1000)
    }

    fn fail(throttle: &FailureThrottle, source: IpAddr, times: usize, at: i64) {
        for _ in 0..times {
            throttle.record_failure(source, at);
        }
    }

    #[test]
    fn nine_failures_are_tolerated_and_the_tenth_trips() {
        let t = throttle();
        fail(&t, ip(1), 9, 1_000);
        assert!(t.check(ip(1), 1_000).is_ok());
        fail(&t, ip(1), 1, 1_000);
        let err = t.check(ip(1), 1_000).unwrap_err();
        assert_eq!(err.retry_after_secs, 60);
    }

    #[test]
    fn retry_after_counts_down_and_never_drops_below_one() {
        let t = throttle();
        fail(&t, ip(1), 10, 0);
        assert_eq!(t.check(ip(1), 30_000).unwrap_err().retry_after_secs, 30);
        assert_eq!(t.check(ip(1), 59_001).unwrap_err().retry_after_secs, 1);
        assert_eq!(t.check(ip(1), 59_999).unwrap_err().retry_after_secs, 1);
    }

    #[test]
    fn it_recovers_exactly_when_the_window_ends() {
        let t = throttle();
        fail(&t, ip(1), 10, 0);
        assert!(t.check(ip(1), WINDOW - 1).is_err());
        assert!(t.check(ip(1), WINDOW).is_ok());
    }

    #[test]
    fn failures_spread_over_more_than_a_window_never_trip() {
        let t = throttle();
        for n in 0..50 {
            t.record_failure(ip(1), n * 7_000);
            assert!(t.check(ip(1), n * 7_000).is_ok(), "failure {n}");
        }
    }

    #[test]
    fn pushing_a_throttled_address_does_not_extend_the_block() {
        let t = throttle();
        fail(&t, ip(1), 10, 0);
        fail(&t, ip(1), 500, 30_000);
        assert!(t.check(ip(1), WINDOW).is_ok());
    }

    #[test]
    fn addresses_are_independent() {
        let t = throttle();
        fail(&t, ip(1), 10, 0);
        assert!(t.check(ip(1), 0).is_err());
        assert!(t.check(ip(2), 0).is_ok());
    }

    #[test]
    fn memory_is_bounded_under_a_flood_of_distinct_addresses() {
        let t = FailureThrottle::new(10, WINDOW, 100);
        for n in 0..10_000u32 {
            t.record_failure(IpAddr::from(n.to_be_bytes()), 0);
        }
        assert_eq!(t.tracked_addresses(), 100);
    }

    #[test]
    fn the_least_recently_failing_address_is_evicted_first() {
        let t = FailureThrottle::new(2, WINDOW, 2);
        fail(&t, ip(1), 2, 0);
        fail(&t, ip(2), 1, 0);
        fail(&t, ip(1), 0, 0);
        t.record_failure(ip(3), 0);
        // ip(1) failed first and is now the stalest of the three.
        assert!(t.check(ip(1), 0).is_ok(), "evicted, so forgiven");
        assert_eq!(t.tracked_addresses(), 2);
    }

    #[test]
    fn expired_entries_are_forgotten_when_checked() {
        let t = throttle();
        t.record_failure(ip(1), 0);
        assert_eq!(t.tracked_addresses(), 1);
        assert!(t.check(ip(1), WINDOW).is_ok());
        assert_eq!(t.tracked_addresses(), 0);
    }
}
