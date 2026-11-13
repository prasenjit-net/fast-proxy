//! Property tests (M6 Q5): the redactor never lets a secret through at any cut of
//! any generated body, a latency window never reports a p99 outside the bucket of
//! the true one, and the two limiters never admit more than their rate allows
//! however the clock readings of concurrent callers interleave.

use proptest::prelude::*;

use fast_proxy_model::Trigger;

use crate::{bucket_of, LatencyWindow, RateLimiter, Redactor, StormGuard, StormKey};

/// No generated text contains this, so any occurrence in the output is a leak.
const LEAK: &str = "ZQ";

/// A name that may or may not be sensitive, in any letter case.
fn field_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("password".to_string()),
        Just("Token".to_string()),
        Just("client_secret".to_string()),
        Just("API_KEY".to_string()),
        "[a-y]{1,8}",
    ]
}

fn is_sensitive(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    ["password", "token", "client_secret", "api_key"].contains(&lower.as_str())
}

/// A secret value: a string, a number, or a nested document holding more.
fn secret_json() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-y ]{0,10}".prop_map(|s| format!("\"{s}ZQ{s}\"")),
        Just("12345ZQ".to_string()),
        Just("{\"x\":[\"ZQ\",{\"y\":\"ZQ\"}]}".to_string()),
        Just("[\"ZQ\",\"ZQ\"]".to_string()),
    ]
}

fn plain_json() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-y ]{0,10}".prop_map(|s| format!("\"{s}\"")),
        (0u32..1000).prop_map(|n| n.to_string()),
        Just("{\"inner\":\"fine\"}".to_string()),
        Just("[1,2,3]".to_string()),
    ]
}

fn json_body() -> impl Strategy<Value = String> {
    prop::collection::vec(
        field_name().prop_flat_map(|name| {
            let value = if is_sensitive(&name) {
                secret_json().boxed()
            } else {
                plain_json().boxed()
            };
            (Just(name), value)
        }),
        1..8,
    )
    .prop_map(|members| {
        let inner: Vec<String> = members
            .iter()
            .map(|(k, v)| format!("\"{k}\":{v}"))
            .collect();
        format!("{{{}}}", inner.join(","))
    })
}

fn form_body() -> impl Strategy<Value = String> {
    prop::collection::vec(
        field_name().prop_flat_map(|name| {
            let value = if is_sensitive(&name) {
                "[a-y]{0,6}".prop_map(|s| format!("{s}ZQ{s}")).boxed()
            } else {
                "[a-y]{0,6}".boxed()
            };
            (Just(name), value)
        }),
        1..8,
    )
    .prop_map(|fields| {
        fields
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&")
    })
}

fn assert_no_cut_leaks(content_type: &str, text: &str) -> Result<(), TestCaseError> {
    let redactor = Redactor::default();
    for cut in (0..=text.len()).filter(|cut| text.is_char_boundary(*cut)) {
        let out = redactor.body(content_type, &text.as_bytes()[..cut], cut < text.len());
        prop_assert!(
            !out.text.contains(LEAK),
            "cut {cut} of {text:?} leaked: {:?}",
            out.text
        );
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn no_cut_of_a_generated_json_body_leaks(text in json_body()) {
        assert_no_cut_leaks("application/json", &text)?;
    }

    #[test]
    fn no_cut_of_a_generated_form_body_leaks(text in form_body()) {
        assert_no_cut_leaks("application/x-www-form-urlencoded", &text)?;
    }

    #[test]
    fn a_secret_query_parameter_never_survives(
        values in prop::collection::vec("[a-y]{0,6}", 1..6),
        secret_at in 0usize..6,
    ) {
        let redactor = Redactor::default();
        let query: Vec<String> = values
            .iter()
            .enumerate()
            .map(|(n, v)| if n == secret_at % values.len() { format!("token={v}ZQ") } else { format!("p{n}={v}") })
            .collect();
        let out = redactor.query(&query.join("&"));
        prop_assert!(!out.contains(LEAK), "{out}");
    }

    #[test]
    fn the_p99_estimate_lies_in_the_bucket_of_the_true_p99(
        latencies in prop::collection::vec(0u32..60_000, 200..1500),
    ) {
        let window = LatencyWindow::new();
        for ms in &latencies {
            window.record(*ms, 1_000);
        }
        window.publish(1_000);
        let mut sorted = latencies.clone();
        sorted.sort_unstable();
        let true_p99 = sorted[(sorted.len() * 99).div_ceil(100) - 1];
        let estimate = window.p99_ms();
        prop_assert!(bucket_of(estimate).abs_diff(bucket_of(true_p99)) <= 1, "estimate {estimate}, true {true_p99}");
    }

    /// However concurrent callers' clock readings interleave (a few milliseconds
    /// out of order), the storm guard keeps at most its burst plus one per
    /// interval of elapsed time.
    #[test]
    fn the_storm_guard_never_keeps_more_than_burst_plus_one_per_interval(
        burst in 1u32..6,
        interval_secs in 1u32..4,
        steps in prop::collection::vec((0u64..40, 0u64..6), 50..400),
    ) {
        let guard = StormGuard::new();
        let key = StormKey::new("shop", "/", Trigger::UpstreamFailure, None);
        let (mut clock, mut kept) = (10_000u64, 0u64);
        for (advance, lateness) in &steps {
            clock += advance;
            let reading = clock.saturating_sub(*lateness);
            if matches!(guard.admit(key, burst, interval_secs, reading), crate::Admission::Keep { .. }) {
                kept += 1;
            }
        }
        let elapsed_ms = clock - 10_000;
        let allowed = u64::from(burst) + elapsed_ms / (u64::from(interval_secs) * 1000) + 1;
        prop_assert!(kept <= allowed, "kept {kept} of {} in {elapsed_ms} ms, allowed {allowed}", steps.len());
    }

    #[test]
    fn the_rate_limiter_never_admits_more_than_burst_plus_the_rate(
        rate in 1u32..50,
        burst in 1u32..50,
        steps in prop::collection::vec((0u64..20, 0u64..6), 50..400),
    ) {
        let limiter = RateLimiter::new();
        let (mut clock, mut admitted) = (10_000u64, 0u64);
        for (advance, lateness) in &steps {
            clock += advance;
            if limiter.try_acquire(clock.saturating_sub(*lateness), rate, burst) {
                admitted += 1;
            }
        }
        let elapsed_ms = clock - 10_000;
        let allowed = u64::from(burst) + elapsed_ms * u64::from(rate) / 1000 + 1;
        prop_assert!(admitted <= allowed, "admitted {admitted} in {elapsed_ms} ms, allowed {allowed}");
    }
}
