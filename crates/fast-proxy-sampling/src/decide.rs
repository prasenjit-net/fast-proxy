//! The sampling decision (ADR 0020): given what is known when a response has
//! finished, which triggers matched. One pure function over atomics, with the
//! clock and the random draw as parameters; it allocates nothing.
//!
//! An upstream's own 5xx is not a trigger (it is the application's answer). A
//! connectivity failure that ended the request is `upstream_failure`; one that
//! was retried to an answer is `retried`.

use std::net::IpAddr;

use fast_proxy_model::{Trigger, TriggerSet};

use crate::client_errors::ClientErrors;
use crate::facts::{Answer, Outcome, UnusualFlags};
use crate::latency_window::LatencyWindow;
use crate::policy::SamplingPolicy;

/// The mutable state of the route and the process the decision reads.
pub struct DecisionState<'a> {
    /// The route's rolling latency.
    pub latency: &'a LatencyWindow,
    /// The per-client 4xx counters of the process.
    pub clients: &'a ClientErrors,
}

/// Decides which triggers a finished request matched. `roll_per_mille` is a
/// uniform draw in `0..1000` for the baseline (injected so tests are exact).
pub fn decide(
    policy: &SamplingPolicy,
    flags: UnusualFlags,
    client: IpAddr,
    outcome: &Outcome,
    state: &DecisionState<'_>,
    roll_per_mille: u32,
    now_ms: u64,
) -> TriggerSet {
    if !policy.enabled {
        return TriggerSet::EMPTY;
    }
    let mut matched = TriggerSet::EMPTY;
    let mut hit = |trigger: Trigger, condition: bool| {
        if condition && policy.triggers.contains(trigger) {
            matched = matched.with(trigger);
        }
    };
    // The proxy's own fixed answers and maintenance say nothing about the
    // upstream; a client that left says nothing about the response.
    let upstream_involved = matches!(outcome.answer, Answer::Upstream | Answer::ProxyFailure);
    let response_complete = !outcome.client_cancelled;

    hit(
        Trigger::UpstreamFailure,
        outcome.answer == Answer::ProxyFailure,
    );
    hit(
        Trigger::Retried,
        outcome.answer == Answer::Upstream && outcome.attempts.connectivity_failures() > 0,
    );
    hit(
        Trigger::Slow,
        upstream_involved && outcome.elapsed_ms >= policy.slow_threshold_ms,
    );
    hit(
        Trigger::SlowP99,
        upstream_involved && is_slow_for_the_route(policy, state.latency, outcome.elapsed_ms),
    );
    hit(
        Trigger::OversizedRequest,
        outcome.bytes_in >= policy.oversized_request_bytes,
    );
    hit(
        Trigger::OversizedResponse,
        response_complete
            && upstream_involved
            && outcome.bytes_out >= policy.oversized_response_bytes,
    );
    hit(Trigger::UnusualHeaders, !flags.is_empty());
    hit(
        Trigger::Baseline,
        roll_per_mille < policy.baseline_per_mille,
    );

    // Counting is state: only a 4xx a client earned, and only when the trigger
    // is on, is counted.
    let client_error = (400..500).contains(&outcome.status)
        && response_complete
        && outcome.answer != Answer::Maintenance
        && policy.triggers.contains(Trigger::ClientErrors);
    if client_error {
        let verdict = state
            .clients
            .record(client, now_ms, policy.client_errors_per_min);
        hit(Trigger::ClientErrors, verdict.is_sampled());
    }
    matched
}

/// Slower than the route's rolling p99 by the policy's factor, and not below
/// the floor. False while the window has too little to say.
fn is_slow_for_the_route(
    policy: &SamplingPolicy,
    latency: &LatencyWindow,
    elapsed_ms: u32,
) -> bool {
    let p99 = u64::from(latency.p99_ms());
    if p99 == 0 {
        return false;
    }
    let limit =
        (p99 * u64::from(policy.slow_p99_factor_x10) / 10).max(u64::from(policy.slow_p99_floor_ms));
    u64::from(elapsed_ms) >= limit
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::SamplingSettings;

    use super::*;
    use crate::facts::{Attempt, AttemptLog, FailureKind};
    use crate::latency_window::MIN_REQUESTS;

    const NO_ROLL: u32 = 1000;

    fn client() -> IpAddr {
        IpAddr::from([203, 0, 113, 9])
    }

    fn policy() -> SamplingPolicy {
        SamplingPolicy::resolve(&SamplingSettings::default(), None, None)
    }

    fn with_triggers(extra: &[Trigger]) -> SamplingPolicy {
        let mut p = policy();
        for trigger in extra {
            p.triggers = p.triggers.with(*trigger);
        }
        p
    }

    fn ok() -> Outcome {
        Outcome {
            status: 200,
            elapsed_ms: 20,
            answer: Answer::Upstream,
            attempts: attempts(&[(None, Some(200))]),
            bytes_in: 100,
            bytes_out: 500,
            client_cancelled: false,
        }
    }

    fn attempts(list: &[(Option<FailureKind>, Option<u16>)]) -> AttemptLog {
        let mut log = AttemptLog::default();
        for (failure, status) in list {
            log.push(Attempt {
                endpoint_index: 0,
                failure: *failure,
                upstream_status: *status,
            });
        }
        log
    }

    struct World {
        latency: LatencyWindow,
        clients: ClientErrors,
    }

    impl World {
        fn new() -> Self {
            Self {
                latency: LatencyWindow::new(),
                clients: ClientErrors::new(),
            }
        }

        fn decide(
            &self,
            policy: &SamplingPolicy,
            flags: UnusualFlags,
            outcome: &Outcome,
            roll: u32,
        ) -> TriggerSet {
            let state = DecisionState {
                latency: &self.latency,
                clients: &self.clients,
            };
            decide(policy, flags, client(), outcome, &state, roll, 0)
        }

        fn quick(&self, outcome: &Outcome) -> TriggerSet {
            self.decide(&policy(), UnusualFlags::default(), outcome, NO_ROLL)
        }
    }

    fn only(trigger: Trigger) -> TriggerSet {
        [trigger].into_iter().collect()
    }

    #[test]
    fn an_ordinary_request_matches_nothing() {
        assert_eq!(World::new().quick(&ok()), TriggerSet::EMPTY);
    }

    #[test]
    fn a_disabled_policy_matches_nothing_even_for_a_failure() {
        let mut disabled = policy();
        disabled.enabled = false;
        let mut failed = ok();
        failed.answer = Answer::ProxyFailure;
        let got = World::new().decide(&disabled, UnusualFlags::default(), &failed, 0);
        assert_eq!(got, TriggerSet::EMPTY);
    }

    #[test]
    fn an_upstreams_own_5xx_is_not_a_trigger() {
        let world = World::new();
        for status in [500, 502, 503, 504] {
            let mut outcome = ok();
            outcome.status = status;
            outcome.attempts = attempts(&[(None, Some(status))]);
            assert_eq!(
                world.quick(&outcome),
                TriggerSet::EMPTY,
                "upstream {status}"
            );
        }
    }

    #[test]
    fn every_connectivity_failure_that_ends_the_request_is_an_upstream_failure() {
        let world = World::new();
        for kind in [
            FailureKind::ConnectRefused,
            FailureKind::ConnectTimeout,
            FailureKind::TlsHandshake,
            FailureKind::ConnectionReset,
            FailureKind::ResponseTimeout,
        ] {
            let mut outcome = ok();
            outcome.status = if kind == FailureKind::ResponseTimeout {
                504
            } else {
                502
            };
            outcome.answer = Answer::ProxyFailure;
            outcome.attempts = attempts(&[(Some(kind), None)]);
            assert_eq!(
                world.quick(&outcome),
                only(Trigger::UpstreamFailure),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn a_connectivity_failure_that_was_retried_to_an_answer_is_retried() {
        let mut outcome = ok();
        outcome.attempts =
            attempts(&[(Some(FailureKind::ConnectRefused), None), (None, Some(200))]);
        assert_eq!(World::new().quick(&outcome), only(Trigger::Retried));
    }

    #[test]
    fn a_retry_after_an_upstream_503_is_not_retried() {
        let mut outcome = ok();
        outcome.attempts = attempts(&[(None, Some(503)), (None, Some(200))]);
        assert_eq!(World::new().quick(&outcome), TriggerSet::EMPTY);
    }

    #[test]
    fn a_failure_after_an_earlier_failure_is_only_an_upstream_failure() {
        let mut outcome = ok();
        outcome.status = 502;
        outcome.answer = Answer::ProxyFailure;
        outcome.attempts = attempts(&[
            (Some(FailureKind::ConnectRefused), None),
            (Some(FailureKind::ConnectRefused), None),
        ]);
        assert_eq!(World::new().quick(&outcome), only(Trigger::UpstreamFailure));
    }

    #[test]
    fn slow_applies_at_exactly_the_threshold() {
        let world = World::new();
        let mut outcome = ok();
        outcome.elapsed_ms = 1999;
        assert_eq!(world.quick(&outcome), TriggerSet::EMPTY);
        outcome.elapsed_ms = 2000;
        assert_eq!(world.quick(&outcome), only(Trigger::Slow));
    }

    #[test]
    fn the_relative_trigger_is_off_by_default_and_waits_for_a_known_p99() {
        let world = World::new();
        let mut outcome = ok();
        outcome.elapsed_ms = 900;
        let on = with_triggers(&[Trigger::SlowP99]);
        assert_eq!(
            world.decide(&on, UnusualFlags::default(), &outcome, NO_ROLL),
            TriggerSet::EMPTY,
            "no p99 yet"
        );
        for _ in 0..MIN_REQUESTS {
            world.latency.record(100, 1_000);
        }
        world.latency.publish(1_000);
        let p99 = u64::from(world.latency.p99_ms());
        assert!(p99 >= 100);
        assert_eq!(
            world.quick(&outcome),
            TriggerSet::EMPTY,
            "off by default, whatever the window says"
        );
        let limit = p99 * 3;
        outcome.elapsed_ms = (limit - 1) as u32;
        assert_eq!(
            world.decide(&on, UnusualFlags::default(), &outcome, NO_ROLL),
            TriggerSet::EMPTY
        );
        outcome.elapsed_ms = limit as u32;
        assert_eq!(
            world.decide(&on, UnusualFlags::default(), &outcome, NO_ROLL),
            only(Trigger::SlowP99)
        );
    }

    #[test]
    fn the_relative_trigger_never_fires_below_its_floor() {
        let world = World::new();
        for _ in 0..MIN_REQUESTS {
            world.latency.record(2, 1_000);
        }
        world.latency.publish(1_000);
        let on = with_triggers(&[Trigger::SlowP99]);
        let mut outcome = ok();
        outcome.elapsed_ms = 49; // many times the p99, under the 50 ms floor
        assert_eq!(
            world.decide(&on, UnusualFlags::default(), &outcome, NO_ROLL),
            TriggerSet::EMPTY
        );
        outcome.elapsed_ms = 50;
        assert_eq!(
            world.decide(&on, UnusualFlags::default(), &outcome, NO_ROLL),
            only(Trigger::SlowP99)
        );
    }

    #[test]
    fn a_client_is_sampled_when_it_crosses_the_4xx_threshold() {
        let world = World::new();
        let mut p = policy();
        p.client_errors_per_min = 3;
        let mut outcome = ok();
        outcome.status = 404;
        let got: Vec<TriggerSet> = (0..4)
            .map(|_| world.decide(&p, UnusualFlags::default(), &outcome, NO_ROLL))
            .collect();
        assert_eq!(got[0], TriggerSet::EMPTY);
        assert_eq!(got[1], TriggerSet::EMPTY);
        assert_eq!(got[2], only(Trigger::ClientErrors));
        assert_eq!(
            got[3],
            TriggerSet::EMPTY,
            "above the threshold only one in N is sampled"
        );
    }

    #[test]
    fn non_4xx_statuses_and_cancelled_requests_are_not_counted_against_a_client() {
        let world = World::new();
        let mut p = policy();
        p.client_errors_per_min = 1;
        let mut ok_status = ok();
        ok_status.status = 200;
        let mut cancelled = ok();
        cancelled.status = 499;
        cancelled.client_cancelled = true;
        for _ in 0..3 {
            assert_eq!(
                world.decide(&p, UnusualFlags::default(), &ok_status, NO_ROLL),
                TriggerSet::EMPTY
            );
            assert_eq!(
                world.decide(&p, UnusualFlags::default(), &cancelled, NO_ROLL),
                TriggerSet::EMPTY
            );
        }
        let mut error = ok();
        error.status = 400;
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &error, NO_ROLL),
            only(Trigger::ClientErrors),
            "the first real 4xx is the first one counted"
        );
    }

    #[test]
    fn client_errors_are_not_counted_while_the_trigger_is_off() {
        let world = World::new();
        let mut p = policy();
        p.client_errors_per_min = 2;
        p.triggers = TriggerSet::EMPTY;
        let mut error = ok();
        error.status = 404;
        for _ in 0..5 {
            world.decide(&p, UnusualFlags::default(), &error, NO_ROLL);
        }
        p.triggers = only(Trigger::ClientErrors);
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &error, NO_ROLL),
            TriggerSet::EMPTY,
            "the count started now"
        );
    }

    #[test]
    fn oversized_requests_and_responses_apply_at_the_threshold() {
        let world = World::new();
        let p = policy();
        let mut outcome = ok();
        outcome.bytes_in = p.oversized_request_bytes - 1;
        outcome.bytes_out = p.oversized_response_bytes - 1;
        assert_eq!(world.quick(&outcome), TriggerSet::EMPTY);
        outcome.bytes_in = p.oversized_request_bytes;
        assert_eq!(world.quick(&outcome), only(Trigger::OversizedRequest));
        outcome.bytes_out = p.oversized_response_bytes;
        let both = world.quick(&outcome);
        assert!(
            both.contains(Trigger::OversizedRequest) && both.contains(Trigger::OversizedResponse)
        );
    }

    #[test]
    fn unusual_headers_are_a_trigger_on_their_own() {
        let flags = UnusualFlags::default().with(UnusualFlags::HUGE_HEADER_VALUE);
        let got = World::new().decide(&policy(), flags, &ok(), NO_ROLL);
        assert_eq!(got, only(Trigger::UnusualHeaders));
    }

    #[test]
    fn the_baseline_follows_the_draw_exactly() {
        let world = World::new();
        let p = policy(); // 1 per mille
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &ok(), 0),
            only(Trigger::Baseline)
        );
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &ok(), 1),
            TriggerSet::EMPTY
        );
        let mut off = policy();
        off.baseline_per_mille = 0;
        assert_eq!(
            world.decide(&off, UnusualFlags::default(), &ok(), 0),
            TriggerSet::EMPTY
        );
        let mut always = policy();
        always.baseline_per_mille = 1000;
        assert_eq!(
            world.decide(&always, UnusualFlags::default(), &ok(), 999),
            only(Trigger::Baseline)
        );
    }

    #[test]
    fn a_disabled_trigger_never_matches() {
        let world = World::new();
        let mut p = policy();
        p.triggers = TriggerSet::EMPTY;
        let mut failed = ok();
        failed.answer = Answer::ProxyFailure;
        failed.elapsed_ms = 60_000;
        assert_eq!(
            world.decide(
                &p,
                UnusualFlags::default().with(UnusualFlags::HUGE_PATH),
                &failed,
                0
            ),
            TriggerSet::EMPTY
        );
    }

    #[test]
    fn several_triggers_are_all_recorded_in_priority_order() {
        let mut failed = ok();
        failed.status = 504;
        failed.answer = Answer::ProxyFailure;
        failed.elapsed_ms = 30_000;
        failed.attempts = attempts(&[(Some(FailureKind::ResponseTimeout), None)]);
        let flags = UnusualFlags::default().with(UnusualFlags::HUGE_PATH);
        let got = World::new().decide(&policy(), flags, &failed, 0);
        assert_eq!(
            got.iter().collect::<Vec<_>>(),
            [
                Trigger::UpstreamFailure,
                Trigger::Slow,
                Trigger::UnusualHeaders,
                Trigger::Baseline
            ]
        );
        assert_eq!(got.primary(), Some(Trigger::UpstreamFailure));
    }

    #[test]
    fn a_maintenance_answer_is_never_an_upstream_failure_or_slow() {
        let mut maintenance = ok();
        maintenance.status = 503;
        maintenance.answer = Answer::Maintenance;
        maintenance.elapsed_ms = 60_000;
        maintenance.attempts = AttemptLog::default();
        let world = World::new();
        assert_eq!(world.quick(&maintenance), TriggerSet::EMPTY);
        assert_eq!(
            world.decide(&policy(), UnusualFlags::default(), &maintenance, 0),
            only(Trigger::Baseline),
            "the baseline can still pick it"
        );
        let flags = UnusualFlags::default().with(UnusualFlags::UNKNOWN_METHOD);
        assert_eq!(
            world.decide(&policy(), flags, &maintenance, NO_ROLL),
            only(Trigger::UnusualHeaders)
        );
    }

    #[test]
    fn the_unmatched_host_404_is_picked_only_by_the_baseline_and_client_errors() {
        let world = World::new();
        let mut p = policy();
        p.client_errors_per_min = 2;
        let mut fixed = ok();
        fixed.status = 404;
        fixed.answer = Answer::Fixed;
        fixed.attempts = AttemptLog::default();
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &fixed, NO_ROLL),
            TriggerSet::EMPTY
        );
        assert_eq!(
            world.decide(&p, UnusualFlags::default(), &fixed, NO_ROLL),
            only(Trigger::ClientErrors)
        );
        assert!(world
            .decide(&p, UnusualFlags::default(), &fixed, 0)
            .contains(Trigger::Baseline));
        let mut slow = fixed.clone();
        slow.elapsed_ms = 60_000;
        let fresh = World::new();
        assert_eq!(
            fresh.decide(&p, UnusualFlags::default(), &slow, NO_ROLL),
            TriggerSet::EMPTY,
            "slow means nothing for a fixed answer"
        );
    }

    #[test]
    fn a_client_that_left_is_sampled_only_for_other_reasons() {
        let world = World::new();
        let mut gone = ok();
        gone.status = 499;
        gone.client_cancelled = true;
        gone.bytes_out = u64::MAX;
        assert_eq!(
            world.quick(&gone),
            TriggerSet::EMPTY,
            "no oversized response, no client error"
        );
        gone.elapsed_ms = 5_000;
        assert_eq!(world.quick(&gone), only(Trigger::Slow));
        assert!(world
            .decide(&policy(), UnusualFlags::default(), &gone, 0)
            .contains(Trigger::Baseline));
    }
}
