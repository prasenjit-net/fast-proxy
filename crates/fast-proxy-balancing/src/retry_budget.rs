use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use fast_proxy_model::RetryBudget as RetryBudgetPolicy;

/// Seconds of history the budget looks at.
const WINDOW_SECS: u64 = 10;

#[derive(Debug, Default)]
struct Bucket {
    /// Which second this bucket currently holds counts for.
    second: AtomicU64,
    requests: AtomicU64,
    retries: AtomicU64,
}

/// A per-group cap on retries, so a failing group is not hammered by its own
/// retries: over the last ten seconds, retries may not exceed `percent` of the
/// requests, nor fall below `min_per_sec` per second (the floor lets a quiet
/// group retry at all). Lock-free: one bucket per second in a ring.
#[derive(Debug)]
pub struct RetryBudget {
    policy: RetryBudgetPolicy,
    buckets: [Bucket; WINDOW_SECS as usize],
}

impl RetryBudget {
    /// An empty budget under `policy`.
    pub fn new(policy: RetryBudgetPolicy) -> Self {
        Self {
            policy,
            buckets: Default::default(),
        }
    }

    fn bucket(&self, now_ms: u64) -> &Bucket {
        let second = now_ms / 1000;
        let bucket = &self.buckets[(second % WINDOW_SECS) as usize];
        // A stale bucket is recycled by the first caller of its new second;
        // a racing increment may be lost, which only makes the budget slightly
        // more generous.
        if bucket.second.swap(second, Relaxed) != second {
            bucket.requests.store(0, Relaxed);
            bucket.retries.store(0, Relaxed);
        }
        bucket
    }

    /// Counts one first attempt of a request.
    pub fn record_request(&self, now_ms: u64) {
        self.bucket(now_ms).requests.fetch_add(1, Relaxed);
    }

    /// Takes one retry from the budget; `false` means it is spent.
    pub fn try_acquire_retry(&self, now_ms: u64) -> bool {
        let current = now_ms / 1000;
        let (mut requests, mut retries) = (0, 0);
        for bucket in &self.buckets {
            if current.saturating_sub(bucket.second.load(Relaxed)) < WINDOW_SECS {
                requests += bucket.requests.load(Relaxed);
                retries += bucket.retries.load(Relaxed);
            }
        }
        let share = requests * u64::from(self.policy.percent) / 100;
        let floor = u64::from(self.policy.min_per_sec) * WINDOW_SECS;
        if retries >= share.max(floor) {
            return false;
        }
        self.bucket(now_ms).retries.fetch_add(1, Relaxed);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(percent: u32, min_per_sec: u32) -> RetryBudget {
        RetryBudget::new(RetryBudgetPolicy {
            percent,
            min_per_sec,
        })
    }

    #[test]
    fn the_floor_allows_retries_in_a_quiet_group() {
        let budget = budget(20, 1);
        let granted = (0..30).filter(|_| budget.try_acquire_retry(0)).count();
        assert_eq!(granted, 10, "1 per second over a ten second window");
    }

    #[test]
    fn the_percentage_grows_the_allowance_with_traffic() {
        let budget = budget(20, 0);
        for _ in 0..100 {
            budget.record_request(0);
        }
        let granted = (0..100).filter(|_| budget.try_acquire_retry(0)).count();
        assert_eq!(granted, 20);
    }

    #[test]
    fn a_zero_budget_never_retries() {
        let budget = budget(0, 0);
        budget.record_request(0);
        assert!(!budget.try_acquire_retry(0));
    }

    #[test]
    fn old_seconds_fall_out_of_the_window() {
        let budget = budget(0, 1);
        assert_eq!((0..20).filter(|_| budget.try_acquire_retry(0)).count(), 10);
        assert!(!budget.try_acquire_retry(5_000));
        assert!(budget.try_acquire_retry(11_000), "the old retries expired");
    }

    #[test]
    fn counts_in_a_recycled_bucket_start_over() {
        let budget = budget(100, 0);
        budget.record_request(0);
        assert!(budget.try_acquire_retry(0));
        // Ten seconds later the same ring slot is reused.
        budget.record_request(10_000);
        assert!(budget.try_acquire_retry(10_000));
        assert!(
            !budget.try_acquire_retry(10_000),
            "one request allows one retry at 100%"
        );
    }
}
