//! The guarantee a rate limit makes, for any policy and any order of clock
//! readings (concurrent callers read the clock before they reach the table).

use std::time::Duration;

use fast_proxy_model::{RateLimitKey, RateLimitPolicy, DEFAULT_IPV6_PREFIX};
use proptest::prelude::*;

use crate::{Gcra, Verdict};

const SECOND: u64 = 1_000_000_000;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Over any window of readings, no more than `burst + rate * elapsed / period`
    /// requests (rounded up) are allowed.
    #[test]
    fn never_more_than_the_burst_plus_the_rate(
        rate in 1u32..50,
        period_secs in 1u64..5,
        burst in 1u32..20,
        offsets in prop::collection::vec(0u64..(10 * SECOND), 1..300),
    ) {
        let gcra = Gcra::new(&RateLimitPolicy {
            rate,
            period: Duration::from_secs(period_secs),
            burst,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        });
        let start = SECOND;
        let mut tat = 0;
        let mut allowed = 0u64;
        let (mut first, mut last) = (u64::MAX, 0);
        for offset in &offsets {
            let now = start + offset;
            first = first.min(now);
            last = last.max(now);
            if let Verdict::Allow { next_tat } = gcra.check(tat, now) {
                tat = next_tat;
                allowed += 1;
            }
        }
        let elapsed = last - first;
        let period_ns = period_secs * SECOND;
        let steady = (u64::from(rate) * elapsed).div_ceil(period_ns);
        prop_assert!(
            allowed <= u64::from(burst) + steady + 1,
            "{allowed} allowed over {elapsed} ns at {rate}/{period_secs}s burst {burst}"
        );
    }
}
