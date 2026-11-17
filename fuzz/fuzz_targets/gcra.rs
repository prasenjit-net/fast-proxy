//! The rate limiter and the sampler's token bucket with arbitrary clock
//! readings, out of order included (M7 5.4; the M6 bucket bug was exactly this).
#![no_main]

use std::sync::atomic::AtomicU64;
use std::time::Duration;

use fast_proxy_limits::{bucket, Admission, Gcra, KeyBudget, KeyTable};
use fast_proxy_model::{RateLimitKey, RateLimitPolicy, DEFAULT_IPV6_PREFIX};
use libfuzzer_sys::{arbitrary, fuzz_target};

#[derive(Debug, arbitrary::Arbitrary)]
struct Input {
    rate: u32,
    period_secs: u8,
    burst: u16,
    /// Key (mod 4) and clock reading in nanoseconds, in any order.
    checks: Vec<(u8, u64)>,
    bucket_cap: u8,
    takes: Vec<u32>,
}

fuzz_target!(|input: Input| {
    let policy = RateLimitPolicy {
        rate: input.rate,
        period: Duration::from_secs(u64::from(input.period_secs)),
        burst: u32::from(input.burst),
        key: RateLimitKey::Client,
        ipv6_prefix: DEFAULT_IPV6_PREFIX,
    };
    if policy.validate().is_ok() {
        let gcra = Gcra::new(&policy);
        let table = KeyTable::new(KeyBudget::new(2));
        let mut allowed_at_zero = [0u32; 4];
        for (key, now) in &input.checks {
            let key = u64::from(key % 4);
            let admission = table.check(&gcra, key, *now);
            if *now == 0 && matches!(admission, Admission::Allowed) {
                allowed_at_zero[key as usize] += 1;
            }
        }
        for allowed in allowed_at_zero {
            assert!(
                allowed <= policy.burst,
                "at one instant a key gets at most its burst"
            );
        }
    }

    // A bucket that never refills gives out at most what it started with,
    // whatever the clock says.
    let cap = u32::from(input.bucket_cap);
    let state = AtomicU64::new(0);
    let mut given = 0u32;
    for now in &input.takes {
        if bucket::take(&state, u64::from(*now), (cap, cap), |_| 0, 1) {
            given += 1;
        }
    }
    assert!(given <= cap, "{given} taken from a bucket of {cap}");
});
