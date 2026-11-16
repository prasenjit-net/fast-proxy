//! GCRA, the generic cell rate algorithm (ADR 0030): a token bucket expressed as
//! one number per key, the *theoretical arrival time* (TAT) of the next request
//! at the steady rate. A request is allowed while the TAT is no more than the
//! burst tolerance ahead of now; each allowed request pushes it one interval on.

use std::time::Duration;

use fast_proxy_model::RateLimitPolicy;

/// A policy reduced to the two numbers the algorithm needs, in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gcra {
    /// Time between requests at the steady rate.
    interval_ns: u64,
    /// How far ahead of now the TAT may run: `burst - 1` intervals.
    tolerance_ns: u64,
}

/// The decision for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Allowed; the key's TAT becomes `next_tat`.
    Allow { next_tat: u64 },
    /// Refused; a retry after `retry_after` would be allowed.
    Deny { retry_after: Duration },
}

impl Gcra {
    /// The algorithm for a validated policy.
    pub fn new(policy: &RateLimitPolicy) -> Self {
        let period_ns = u64::try_from(policy.period.as_nanos()).unwrap_or(u64::MAX);
        let interval_ns = (period_ns / u64::from(policy.rate.max(1))).max(1);
        let tolerance_ns = interval_ns.saturating_mul(u64::from(policy.burst.max(1) - 1));
        Self {
            interval_ns,
            tolerance_ns,
        }
    }

    /// Decides a request at `now` for a key whose stored TAT is `tat` (0 for a
    /// key never seen). A TAT in the past counts as now, so a clock reading
    /// older than the stored one (another thread got there first) never adds
    /// credit, and a key idle long enough is the same as a new one.
    pub fn check(&self, tat: u64, now: u64) -> Verdict {
        let tat = tat.max(now);
        let ahead = tat - now;
        if ahead > self.tolerance_ns {
            return Verdict::Deny {
                retry_after: Duration::from_nanos(ahead - self.tolerance_ns),
            };
        }
        Verdict::Allow {
            next_tat: tat.saturating_add(self.interval_ns),
        }
    }

    /// Whether a key with this stored TAT would be answered exactly like a new
    /// key at `now`, so its entry can be dropped without changing any answer.
    pub fn is_idle(tat: u64, now: u64) -> bool {
        tat <= now
    }
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{RateLimitKey, DEFAULT_IPV6_PREFIX};

    use super::*;

    const SECOND: u64 = 1_000_000_000;

    fn gcra(rate: u32, period_secs: u64, burst: u32) -> Gcra {
        Gcra::new(&RateLimitPolicy {
            rate,
            period: Duration::from_secs(period_secs),
            burst,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        })
    }

    /// Runs `count` requests at `now`, returning how many were allowed.
    fn burst_at(limit: &Gcra, tat: &mut u64, now: u64, count: u32) -> u32 {
        let mut allowed = 0;
        for _ in 0..count {
            if let Verdict::Allow { next_tat } = limit.check(*tat, now) {
                *tat = next_tat;
                allowed += 1;
            }
        }
        allowed
    }

    #[test]
    fn a_burst_is_allowed_at_once_and_then_the_rate() {
        let limit = gcra(10, 1, 5);
        let mut tat = 0;
        assert_eq!(burst_at(&limit, &mut tat, SECOND, 20), 5, "the burst");
        assert_eq!(
            burst_at(&limit, &mut tat, SECOND + SECOND / 10, 5),
            1,
            "one interval later"
        );
        assert_eq!(
            burst_at(&limit, &mut tat, 3 * SECOND, 20),
            5,
            "idle long enough: a new burst"
        );
    }

    #[test]
    fn a_refusal_says_when_a_retry_succeeds() {
        let limit = gcra(2, 1, 1);
        let mut tat = 0;
        assert_eq!(burst_at(&limit, &mut tat, SECOND, 1), 1);
        let Verdict::Deny { retry_after } = limit.check(tat, SECOND) else {
            panic!("the second request in the same instant is refused");
        };
        assert_eq!(retry_after, Duration::from_millis(500));
        assert!(matches!(
            limit.check(tat, SECOND + SECOND / 2),
            Verdict::Allow { .. }
        ));
    }

    #[test]
    fn an_older_clock_reading_adds_no_credit() {
        let limit = gcra(1, 1, 1);
        let mut tat = 0;
        assert_eq!(burst_at(&limit, &mut tat, 10 * SECOND, 1), 1);
        assert_eq!(
            burst_at(&limit, &mut tat, 9 * SECOND, 3),
            0,
            "a late reading is refused"
        );
    }

    #[test]
    fn a_key_is_idle_once_its_time_has_passed() {
        let limit = gcra(1, 1, 3);
        let mut tat = 0;
        burst_at(&limit, &mut tat, SECOND, 3);
        assert!(!Gcra::is_idle(tat, SECOND));
        assert!(Gcra::is_idle(tat, tat));
        assert_eq!(
            limit.check(tat, tat),
            limit.check(0, tat),
            "the same answer as a new key"
        );
    }

    #[test]
    fn slow_rates_and_fast_rates_keep_their_interval() {
        assert_eq!(gcra(1, 3600, 1).interval_ns, 3600 * SECOND);
        assert_eq!(gcra(1_000_000, 1, 1).interval_ns, 1_000);
    }
}
