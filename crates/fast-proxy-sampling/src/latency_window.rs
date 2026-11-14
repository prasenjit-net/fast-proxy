//! A route's rolling latency (decision J5): a log-linear histogram of atomics
//! over two one-minute epochs. Recording is one `fetch_add`; a tick every few
//! seconds computes the p99 and publishes it as one atomic, so the request
//! path only reads an integer. Not a metric, not stored, not shared between
//! instances.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const EPOCH_MS: u64 = 60_000;
const BUCKETS: usize = 64;
/// The p99 is published only once this many requests are in the window.
pub const MIN_REQUESTS: u64 = 200;

/// The histogram bucket of a latency: sub-millisecond latencies are bucket 0,
/// every power of two is split in three, anything beyond the last bucket
/// shares it.
pub fn bucket_of(ms: u32) -> usize {
    if ms == 0 {
        return 0;
    }
    let octave = 31 - ms.leading_zeros();
    let base = 1u64 << octave;
    let third = ((u64::from(ms) - base) * 3 / base) as usize;
    (1 + octave as usize * 3 + third).min(BUCKETS - 1)
}

/// The largest latency, in milliseconds, that falls in `bucket`.
pub fn bucket_upper_ms(bucket: usize) -> u32 {
    if bucket == 0 {
        return 0;
    }
    let octave = (bucket - 1) / 3;
    let third = ((bucket - 1) % 3) as u64;
    let base = 1u64 << octave;
    let upper = base + (base * (third + 1)).div_ceil(3) - 1;
    u32::try_from(upper).unwrap_or(u32::MAX)
}

struct Epoch {
    /// The epoch number plus one; 0 means the slot was never used.
    number: AtomicU64,
    buckets: [AtomicU32; BUCKETS],
}

impl Epoch {
    fn new() -> Self {
        Self {
            number: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }
}

/// The rolling window of one route.
pub struct LatencyWindow {
    epochs: [Epoch; 2],
    p99_ms: AtomicU32,
}

impl Default for LatencyWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LatencyWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LatencyWindow")
            .field("p99_ms", &self.p99_ms())
            .finish()
    }
}

impl LatencyWindow {
    pub fn new() -> Self {
        Self {
            epochs: [Epoch::new(), Epoch::new()],
            p99_ms: AtomicU32::new(0),
        }
    }

    /// Records one request's latency. A request that races the start of a new
    /// epoch may be lost; that is one sample in a statistic.
    pub fn record(&self, elapsed_ms: u32, now_ms: u64) {
        let number = now_ms / EPOCH_MS + 1;
        let epoch = &self.epochs[(number % 2) as usize];
        if epoch.number.load(Ordering::Relaxed) != number
            && epoch.number.swap(number, Ordering::Relaxed) != number
        {
            for bucket in &epoch.buckets {
                bucket.store(0, Ordering::Relaxed);
            }
        }
        epoch.buckets[bucket_of(elapsed_ms)].fetch_add(1, Ordering::Relaxed);
    }

    /// Computes the p99 of the current and the previous epoch and publishes it
    /// (0 while fewer than [`MIN_REQUESTS`] are in the window). Called by a
    /// periodic task, never on the request path.
    pub fn publish(&self, now_ms: u64) {
        let current = now_ms / EPOCH_MS + 1;
        let mut counts = [0u64; BUCKETS];
        for epoch in &self.epochs {
            let number = epoch.number.load(Ordering::Relaxed);
            if number == current || number + 1 == current {
                for (total, bucket) in counts.iter_mut().zip(&epoch.buckets) {
                    *total += u64::from(bucket.load(Ordering::Relaxed));
                }
            }
        }
        let total: u64 = counts.iter().sum();
        if total < MIN_REQUESTS {
            self.p99_ms.store(0, Ordering::Relaxed);
            return;
        }
        let rank = (total * 99).div_ceil(100);
        let mut seen = 0;
        for (bucket, count) in counts.iter().enumerate() {
            seen += count;
            if seen >= rank {
                self.p99_ms
                    .store(bucket_upper_ms(bucket), Ordering::Relaxed);
                return;
            }
        }
    }

    /// The last published p99 in milliseconds; 0 means unknown.
    pub fn p99_ms(&self) -> u32 {
        self.p99_ms.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn buckets_are_ordered_and_cover_the_range() {
        let mut last = 0;
        for ms in [
            0u32,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            100,
            1000,
            60_000,
            600_000,
            u32::MAX,
        ] {
            let bucket = bucket_of(ms);
            assert!(bucket >= last, "{ms} ms went backwards");
            assert!(bucket < BUCKETS);
            last = bucket;
        }
        assert_eq!(
            bucket_of(u32::MAX),
            BUCKETS - 1,
            "the long tail shares the last bucket"
        );
        assert!(
            bucket_of(600_000) < BUCKETS - 1,
            "ten minutes still has its own bucket"
        );
    }

    #[test]
    fn every_latency_lies_inside_its_bucket() {
        for ms in 1..5000u32 {
            let bucket = bucket_of(ms);
            assert!(
                bucket_upper_ms(bucket) >= ms,
                "{ms} ms above its bucket's top"
            );
            if bucket > 1 {
                assert!(
                    bucket_upper_ms(bucket - 1) < ms,
                    "{ms} ms below its bucket's bottom"
                );
            }
        }
    }

    #[test]
    fn nothing_is_published_until_enough_requests_were_seen() {
        let window = LatencyWindow::new();
        for _ in 0..(MIN_REQUESTS - 1) {
            window.record(10, 1_000);
        }
        window.publish(2_000);
        assert_eq!(window.p99_ms(), 0);
        window.record(10, 1_000);
        window.publish(2_000);
        assert!(window.p99_ms() >= 10);
    }

    #[test]
    fn the_estimate_is_in_the_bucket_of_the_true_p99() {
        // A deterministic pseudo-random spread of latencies, several seeds.
        for seed in 1..=20u64 {
            let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let mut latencies: Vec<u32> = (0..2000)
                .map(|_| {
                    state = state
                        .wrapping_mul(6_364_136_223_846_793_005)
                        .wrapping_add(1);
                    let r = (state >> 33) as u32;
                    // Mostly fast, with a heavy tail.
                    if r.is_multiple_of(50) {
                        500 + r % 20_000
                    } else {
                        1 + r % 120
                    }
                })
                .collect();
            let window = LatencyWindow::new();
            for ms in &latencies {
                window.record(*ms, 5_000);
            }
            window.publish(5_000);
            latencies.sort_unstable();
            let rank = (latencies.len() * 99).div_ceil(100);
            let truth = latencies[rank - 1];
            assert_eq!(
                window.p99_ms(),
                bucket_upper_ms(bucket_of(truth)),
                "seed {seed}: true p99 {truth} ms"
            );
        }
    }

    #[test]
    fn an_old_epoch_is_forgotten() {
        let window = LatencyWindow::new();
        for _ in 0..300 {
            window.record(5000, 10_000); // epoch 0: slow
        }
        window.publish(10_000);
        let slow = window.p99_ms();
        assert!(slow >= 5000);
        // One epoch later the slow ones are still in the window.
        for _ in 0..300 {
            window.record(5, 70_000); // epoch 1: fast
        }
        window.publish(70_000);
        assert!(window.p99_ms() >= 5000, "the previous epoch still counts");
        // Two epochs later they are gone.
        for _ in 0..300 {
            window.record(5, 130_000);
        }
        window.publish(130_000);
        assert!(
            window.p99_ms() < 20,
            "only the fast epoch is left, got {}",
            window.p99_ms()
        );
    }

    #[test]
    fn an_idle_window_stops_publishing_a_stale_figure() {
        let window = LatencyWindow::new();
        for _ in 0..300 {
            window.record(900, 1_000);
        }
        window.publish(1_000);
        assert!(window.p99_ms() > 0);
        window.publish(1_000 + 5 * EPOCH_MS);
        assert_eq!(
            window.p99_ms(),
            0,
            "five minutes of silence leave nothing to measure"
        );
    }

    #[test]
    fn concurrent_recording_loses_nothing_within_an_epoch() {
        let window = Arc::new(LatencyWindow::new());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let window = window.clone();
                std::thread::spawn(move || {
                    for i in 0..5000u32 {
                        window.record(i % 100, 3_000);
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let total: u64 = window.epochs[((3_000 / EPOCH_MS + 1) % 2) as usize]
            .buckets
            .iter()
            .map(|b| u64::from(b.load(Ordering::Relaxed)))
            .sum();
        assert_eq!(total, 8 * 5000);
    }
}
