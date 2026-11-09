//! The passive health state machine: consecutive failures eject an endpoint
//! for an exponentially growing time; after it expires one trial request
//! decides recovery. Takes `now` explicitly so tests need no sleeping.

use std::sync::atomic::Ordering::Relaxed;

use fast_proxy_model::PassiveHealth;

use crate::Candidate;

/// How a forwarded request went, as far as endpoint health is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// A connect error, response timeout or 502/503/504.
    Failure,
}

/// A change worth reporting (events, metrics, logs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    /// The endpoint was taken out of rotation until `until_ms`; this was its
    /// `times`-th ejection since it last recovered.
    Ejected { until_ms: u64, times: u32 },
    /// A trial request succeeded and the endpoint is back.
    Recovered,
}

fn ejection_length_ms(policy: &PassiveHealth, times_before: u32) -> u64 {
    let base = u64::try_from(policy.base_ejection.as_millis()).unwrap_or(u64::MAX);
    let max = u64::try_from(policy.max_ejection.as_millis()).unwrap_or(u64::MAX);
    base.saturating_mul(1_u64 << times_before.min(32)).min(max)
}

/// Records the outcome of a request on `group[index]` and returns the
/// transition it caused, if any. A group never ejects more than
/// `max_ejection_percent` of its endpoints, so a single-endpoint group is
/// never ejected.
pub fn record_outcome(
    group: &[Candidate],
    index: usize,
    outcome: Outcome,
    policy: &PassiveHealth,
    now_ms: u64,
) -> Option<Transition> {
    let runtime = &group[index].runtime;
    runtime.trial_in_flight.store(false, Relaxed);
    let until = runtime.ejected_until_ms.load(Relaxed);
    let half_open = until != 0 && now_ms >= until;
    // A straggler that started before the ejection says nothing new.
    if until != 0 && now_ms < until {
        return None;
    }
    match outcome {
        Outcome::Success => {
            runtime.consecutive_failures.store(0, Relaxed);
            if !half_open {
                return None;
            }
            runtime.ejected_until_ms.store(0, Relaxed);
            runtime.ejection_count.store(0, Relaxed);
            Some(Transition::Recovered)
        }
        Outcome::Failure => {
            if policy.consecutive_failures == 0 {
                return None;
            }
            let failures = runtime.consecutive_failures.fetch_add(1, Relaxed) + 1;
            if !half_open && failures < policy.consecutive_failures {
                return None;
            }
            if !half_open && !may_eject_another(group, index, policy) {
                return None;
            }
            let times = runtime.ejection_count.fetch_add(1, Relaxed);
            let until_ms = now_ms
                .saturating_add(ejection_length_ms(policy, times))
                .max(1);
            runtime.ejected_until_ms.store(until_ms, Relaxed);
            runtime.consecutive_failures.store(0, Relaxed);
            Some(Transition::Ejected {
                until_ms,
                times: times + 1,
            })
        }
    }
}

/// Whether ejecting `group[index]` keeps the ejected share within the limit.
fn may_eject_another(group: &[Candidate], index: usize, policy: &PassiveHealth) -> bool {
    let others_ejected = group
        .iter()
        .enumerate()
        .filter(|(i, c)| *i != index && c.runtime.ejected_until_ms.load(Relaxed) != 0)
        .count();
    (others_ejected + 1) * 100 <= policy.max_ejection_percent as usize * group.len()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;
    use crate::EndpointRuntime;

    fn group(size: usize) -> Vec<Candidate> {
        (0..size)
            .map(|_| Candidate {
                runtime: Arc::new(EndpointRuntime::new()),
                weight: 1,
                admin_enabled: true,
            })
            .collect()
    }

    fn policy() -> PassiveHealth {
        PassiveHealth {
            consecutive_failures: 3,
            base_ejection: Duration::from_secs(10),
            max_ejection: Duration::from_secs(40),
            max_ejection_percent: 50,
        }
    }

    fn fail(group: &[Candidate], index: usize, now: u64) -> Option<Transition> {
        record_outcome(group, index, Outcome::Failure, &policy(), now)
    }

    #[test]
    fn consecutive_failures_eject_and_a_success_in_between_resets_the_count() {
        let g = group(3);
        assert_eq!(fail(&g, 0, 0), None);
        assert_eq!(fail(&g, 0, 0), None);
        assert_eq!(record_outcome(&g, 0, Outcome::Success, &policy(), 0), None);
        assert_eq!(fail(&g, 0, 0), None);
        assert_eq!(fail(&g, 0, 0), None);
        assert_eq!(
            fail(&g, 0, 1_000),
            Some(Transition::Ejected {
                until_ms: 11_000,
                times: 1
            })
        );
        assert!(g[0].runtime.is_ejected(10_999) && !g[0].runtime.is_routable(10_999));
    }

    #[test]
    fn after_expiry_one_trial_decides_and_success_recovers_and_resets_backoff() {
        let g = group(3);
        for _ in 0..3 {
            fail(&g, 0, 0);
        }
        assert!(g[0].runtime.is_routable(10_000));
        g[0].runtime.mark_selected(10_000);
        assert!(!g[0].runtime.is_routable(10_000));
        assert_eq!(
            record_outcome(&g, 0, Outcome::Success, &policy(), 10_050),
            Some(Transition::Recovered)
        );
        assert!(g[0].runtime.is_routable(10_050) && !g[0].runtime.is_ejected(10_050));
        for _ in 0..2 {
            fail(&g, 0, 20_000);
        }
        assert_eq!(
            fail(&g, 0, 20_000),
            Some(Transition::Ejected {
                until_ms: 30_000,
                times: 1
            }),
            "backoff restarted at the base after recovery"
        );
    }

    #[test]
    fn a_failed_trial_ejects_again_for_twice_as_long_up_to_the_cap() {
        let g = group(3);
        for _ in 0..3 {
            fail(&g, 0, 0);
        }
        let mut now = 10_000;
        let mut lengths = Vec::new();
        for _ in 0..4 {
            g[0].runtime.mark_selected(now);
            let Some(Transition::Ejected { until_ms, .. }) = fail(&g, 0, now) else {
                panic!("trial failure must eject");
            };
            lengths.push(until_ms - now);
            now = until_ms;
        }
        assert_eq!(lengths, [20_000, 40_000, 40_000, 40_000]);
    }

    #[test]
    fn outcomes_during_an_ejection_are_ignored() {
        let g = group(3);
        for _ in 0..3 {
            fail(&g, 0, 0);
        }
        assert_eq!(
            record_outcome(&g, 0, Outcome::Success, &policy(), 5_000),
            None
        );
        assert_eq!(fail(&g, 0, 5_000), None);
        assert!(g[0].runtime.is_ejected(9_999));
    }

    #[test]
    fn the_guard_never_ejects_more_than_the_allowed_share() {
        let g = group(4); // 50% of 4 = 2
        for index in 0..4 {
            for _ in 0..3 {
                fail(&g, index, 0);
            }
        }
        let ejected = g.iter().filter(|c| c.runtime.is_ejected(1)).count();
        assert_eq!(ejected, 2);
    }

    #[test]
    fn a_single_endpoint_is_never_ejected() {
        let g = group(1);
        for _ in 0..20 {
            assert_eq!(fail(&g, 0, 0), None);
        }
        assert!(g[0].runtime.is_routable(0));
    }

    #[test]
    fn zero_threshold_turns_passive_ejection_off() {
        let g = group(3);
        let off = PassiveHealth {
            consecutive_failures: 0,
            ..policy()
        };
        for _ in 0..20 {
            assert_eq!(record_outcome(&g, 0, Outcome::Failure, &off, 0), None);
        }
        assert!(g[0].runtime.is_routable(0));
    }
}
