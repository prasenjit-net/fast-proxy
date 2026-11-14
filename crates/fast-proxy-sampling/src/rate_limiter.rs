//! The per-instance limit on how many samples are kept per second (decision
//! J10): a token bucket, checked after the storm guard and before anything is
//! built. One atomic, no lock, no allocation.

use std::sync::atomic::AtomicU64;

use crate::bucket::{take, UNSET};

/// One credit unit is a thousandth of a token, so rates below one token per
/// millisecond refill smoothly with integer arithmetic.
const MILLI: u32 = 1000;

/// A token bucket. The rate and the burst are given on each call, so a live
/// settings change applies at once.
#[derive(Debug)]
pub struct RateLimiter {
    state: AtomicU64,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            state: AtomicU64::new(UNSET),
        }
    }

    /// Takes one token if there is one: `rate_per_sec` tokens come back every
    /// second, up to `burst`. A new limiter starts with a full burst.
    pub fn try_acquire(&self, now_ms: u64, rate_per_sec: u32, burst: u32) -> bool {
        let cap = burst.saturating_mul(MILLI);
        take(
            &self.state,
            now_ms,
            (cap, cap),
            |elapsed_ms| elapsed_ms.saturating_mul(u64::from(rate_per_sec)),
            MILLI,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;

    #[test]
    fn a_new_limiter_allows_a_full_burst_then_refuses() {
        let limiter = RateLimiter::new();
        let allowed = (0..10).filter(|_| limiter.try_acquire(1_000, 5, 4)).count();
        assert_eq!(allowed, 4);
    }

    #[test]
    fn tokens_come_back_at_the_rate() {
        let limiter = RateLimiter::new();
        for _ in 0..4 {
            assert!(limiter.try_acquire(0, 10, 4));
        }
        assert!(!limiter.try_acquire(0, 10, 4));
        assert!(
            !limiter.try_acquire(99, 10, 4),
            "99 ms is under one token at 10 per second"
        );
        assert!(limiter.try_acquire(100, 10, 4));
        assert!(!limiter.try_acquire(100, 10, 4));
        // A long quiet time refills to the burst and no further.
        let after_quiet = (0..10)
            .filter(|_| limiter.try_acquire(60_000, 10, 4))
            .count();
        assert_eq!(after_quiet, 4);
    }

    #[test]
    fn a_rate_above_one_per_millisecond_still_refills() {
        let limiter = RateLimiter::new();
        for _ in 0..3 {
            assert!(limiter.try_acquire(0, 10_000, 3));
        }
        assert!(!limiter.try_acquire(0, 10_000, 3));
        assert!(
            limiter.try_acquire(1, 10_000, 3),
            "10 tokens per millisecond"
        );
    }

    #[test]
    fn a_zero_rate_allows_only_the_initial_burst() {
        let limiter = RateLimiter::new();
        assert_eq!((0..5).filter(|_| limiter.try_acquire(0, 0, 2)).count(), 2);
        assert!(!limiter.try_acquire(10_000_000, 0, 2));
    }

    #[test]
    fn a_smaller_burst_applies_to_the_next_call() {
        let limiter = RateLimiter::new();
        assert!(limiter.try_acquire(0, 1, 10));
        let allowed = (0..10).filter(|_| limiter.try_acquire(0, 1, 2)).count();
        assert_eq!(
            allowed, 2,
            "credit is capped to the new burst before it is spent"
        );
    }

    #[test]
    fn concurrent_callers_never_exceed_the_budget() {
        let limiter = Arc::new(RateLimiter::new());
        let granted = Arc::new(AtomicU32::new(0));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (limiter, granted) = (limiter.clone(), granted.clone());
                std::thread::spawn(move || {
                    for _ in 0..1000 {
                        if limiter.try_acquire(5, 1, 50) {
                            granted.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(
            granted.load(Ordering::Relaxed),
            50,
            "exactly the burst, no more, no fewer"
        );
    }
}
