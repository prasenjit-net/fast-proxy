//! Properties of the eligibility rule and the renewal policy over random input.

use std::collections::HashSet;

use fast_proxy_model::{AcmeStatus, DomainAllowlist, HostPattern, SubsystemId};
use proptest::prelude::*;

use crate::{eligible_names, Decision, FailureKind, RenewalFacts, RenewalPolicy};

const DAY: i64 = 86_400_000;
const HOUR: i64 = 3_600_000;

fn host() -> impl Strategy<Value = HostPattern> {
    let label = "[a-z][a-z0-9]{0,6}";
    prop_oneof![
        label.prop_map(|l| HostPattern::parse(&l).unwrap()),
        (label, prop_oneof![Just("example.com"), Just("other.net")])
            .prop_map(|(l, d)| HostPattern::parse(&format!("{l}.{d}")).unwrap()),
        (label, label)
            .prop_map(|(a, b)| HostPattern::parse(&format!("{a}.{b}.example.com")).unwrap()),
        label.prop_map(|l| HostPattern::parse(&format!("*.{l}.example.com")).unwrap()),
    ]
}

fn failure_kind() -> impl Strategy<Value = FailureKind> {
    prop_oneof![
        Just(FailureKind::Transient),
        Just(FailureKind::Permanent),
        proptest::option::of(0i64..10 * HOUR)
            .prop_map(|retry_at_ms| FailureKind::RateLimited { retry_at_ms }),
    ]
}

proptest! {
    #[test]
    fn every_distinct_host_is_eligible_or_skipped_exactly_once(
        hosts in proptest::collection::vec(host(), 0..150)
    ) {
        let allowlist = DomainAllowlist::parse(&["*.example.com".to_string()]).unwrap();
        let result = eligible_names(&hosts, &allowlist);
        let distinct: HashSet<String> = hosts.iter().map(HostPattern::to_pattern_string).collect();
        let mut seen: Vec<&str> = result.eligible.iter().map(String::as_str).collect();
        seen.extend(result.skipped.iter().map(|s| s.name.as_str()));
        prop_assert_eq!(seen.len(), distinct.len());
        prop_assert_eq!(seen.into_iter().collect::<HashSet<_>>().len(), distinct.len());
        prop_assert!(result.eligible.len() <= 100);
        prop_assert!(result.eligible.iter().all(|name| allowlist.allows(name)));
    }

    #[test]
    fn a_certificate_is_due_inside_its_jitter_band_and_before_it_expires(
        not_before in -1_000 * DAY..1_000 * DAY,
        lifetime in 1i64..400 * DAY,
        subsystem in any::<i64>(),
    ) {
        let policy = RenewalPolicy::default();
        let due = policy.due_at(not_before, not_before + lifetime, SubsystemId(subsystem));
        let latest = not_before + lifetime * 667 / 1000;
        let earliest = latest - lifetime * 50 / 1000;
        prop_assert!(due >= earliest - 1 && due <= latest, "{due} not in {earliest}..={latest}");
        prop_assert!(due < not_before + lifetime);
    }

    #[test]
    fn the_next_attempt_is_never_sooner_than_the_shortest_backoff(
        failures in proptest::collection::vec((failure_kind(), 0i64..3 * HOUR), 1..20),
    ) {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(3));
        let mut now = 0;
        for (kind, gap) in failures {
            now += gap;
            policy.record_failure(&mut status, kind, "x", now);
            let next = status.next_attempt_at_ms.unwrap();
            prop_assert!(next - now >= policy.backoff_ms[0] * 9 / 10, "{} after {now}", next - now);
        }
    }

    #[test]
    fn forced_or_scheduled_no_window_ever_holds_more_failures_than_the_cap(
        attempts in proptest::collection::vec((any::<bool>(), 0i64..20 * 60_000), 1..60),
    ) {
        // The runner orders only when `decide` says so: a scheduled attempt at
        // the time the status names, a forced one whenever an admin asks.
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(5));
        let eligible = vec!["shop.example.com".to_string()];
        let mut now = 0;
        for (forced, gap) in attempts {
            now += gap;
            if !forced {
                now = status.next_attempt_at_ms.map_or(now, |next| next.max(now));
            }
            let decision = policy.decide(&RenewalFacts {
                now_ms: now,
                subsystem: SubsystemId(5),
                eligible: &eligible,
                certificate: None,
                status: &status,
                forced,
                names_changed_at_ms: None,
            });
            if matches!(decision, Decision::Order(_)) {
                policy.record_failure(&mut status, FailureKind::Transient, "x", now);
            }
            prop_assert!(
                status.window_failures <= policy.cap_failures,
                "{} failures in one window", status.window_failures
            );
        }
    }
}
