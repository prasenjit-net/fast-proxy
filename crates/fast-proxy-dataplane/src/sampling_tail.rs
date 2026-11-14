//! The end of a sampled request: when the response has finished (or the client
//! left), decide whether the request was unusual, and if it was, hand a raw
//! sample to the sampler. Everything here runs after the response, never
//! before, and never blocks: the storm guard, the rate limit and the hand-off
//! are atomics and a non-blocking queue.
//!
//! The pieces gathered while the request was in flight (the selected headers,
//! the attempt log, the body observers) travel in a [`SamplingTail`], which is
//! moved from the pending request to the tracked response body.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use fast_proxy_balancing::monotonic_ms;
use fast_proxy_model::Trigger;
use fast_proxy_routing::{CompiledUpstream, Snapshot};
use fast_proxy_sampling::{
    decide, Admission, Answer, AttemptDetail, AttemptLog, BodyBudget, BodyReservation,
    CapturedBody, DecisionState, FailureKind, LatencyWindow, Outcome, RawSample, RequestHead,
    Sampler, SamplingPolicy, StormKey, Suppressed,
};
use fast_proxy_telemetry::{DataPlaneFields, Metrics, SampleDropReason};
use http::{HeaderMap, Method, Uri};
use rand::Rng;

use crate::headers;
use crate::replay_body::{BodyObservation, BodyStats};
use crate::sampling_head::{capturable_content_type, HeadCapture, ResponseCapture};

/// A window with nothing in it, for routes that have none (a request no route
/// took): its p99 is unknown, so the relative trigger never fires.
static NO_WINDOW: LazyLock<LatencyWindow> = LazyLock::new(LatencyWindow::new);

/// The start of a response body, kept while it streams to the client.
pub(crate) struct ResponseTee {
    cap: usize,
    captured: Vec<u8>,
    seen: u64,
}

impl ResponseTee {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            captured: Vec::new(),
            seen: 0,
        }
    }

    pub(crate) fn record(&mut self, data: &[u8]) {
        self.seen += data.len() as u64;
        let room = self.cap.saturating_sub(self.captured.len());
        if room > 0 {
            self.captured
                .extend_from_slice(&data[..room.min(data.len())]);
        }
    }

    fn into_body(self, content_type: String) -> CapturedBody {
        CapturedBody {
            content_type,
            truncated: self.seen > self.cap as u64,
            bytes: self.captured,
        }
    }
}

/// What the route, or the lack of one, says about sampling a request.
pub(crate) struct SamplingSource {
    pub(crate) policy: Arc<SamplingPolicy>,
    /// The route's rolling latency, for a request that is forwarded.
    pub(crate) latency: Option<Arc<LatencyWindow>>,
    /// The route's group, for a request that is forwarded.
    pub(crate) upstream: Option<Arc<CompiledUpstream>>,
}

/// Which bodies will be captured for a request (decision J9).
struct BodyPlan {
    reservation: Option<BodyReservation>,
    /// How many request body bytes to keep; 0 only counts.
    request_cap: usize,
    request_content_type: Option<String>,
    omitted: Option<&'static str>,
    /// The memory budget was spent, so the bodies are skipped.
    budget_refused: bool,
}

/// Decides whether bodies are captured for a request. They are, only when the
/// policy asks for it, the request is forwarded, it is not an upgrade, and the
/// process-wide memory budget has room for both caps; the request body only
/// when its type is text, JSON, XML or a form sent uncompressed.
fn plan_bodies(
    policy: &SamplingPolicy,
    forwarded: bool,
    upgrade: bool,
    headers: &HeaderMap,
    budget: &Arc<BodyBudget>,
) -> BodyPlan {
    let none = |omitted| BodyPlan {
        reservation: None,
        request_cap: 0,
        request_content_type: None,
        omitted,
        budget_refused: false,
    };
    if !policy.capture_bodies || !forwarded {
        return none(Some("off"));
    }
    if upgrade {
        return none(Some("upgrade"));
    }
    let wanted = u64::from(policy.body_request_cap) + u64::from(policy.body_response_cap);
    let Some(reservation) = budget.reserve(wanted) else {
        return BodyPlan {
            budget_refused: true,
            ..none(Some("budget"))
        };
    };
    let request_content_type = capturable_content_type(headers);
    let has_body = headers.contains_key(http::header::TRANSFER_ENCODING)
        || headers
            .get(http::header::CONTENT_LENGTH)
            .is_some_and(|length| length.as_bytes() != b"0");
    BodyPlan {
        reservation: Some(reservation),
        request_cap: if request_content_type.is_some() {
            policy.body_request_cap as usize
        } else {
            0
        },
        omitted: (has_body && request_content_type.is_none()).then_some("content_type"),
        request_content_type,
        budget_refused: false,
    }
}

/// What is known about a finished request.
pub(crate) struct Finished<'a> {
    pub(crate) metrics: &'a Metrics,
    pub(crate) sampler: &'a Sampler,
    pub(crate) fields: &'a DataPlaneFields,
    pub(crate) method: &'a str,
    pub(crate) uri: &'a str,
    pub(crate) client: SocketAddr,
    pub(crate) status: u16,
    pub(crate) elapsed: Duration,
    pub(crate) bytes_out: u64,
    /// The client left before the response finished.
    pub(crate) cancelled: bool,
}

/// Everything gathered about a request for the sampling decision.
pub(crate) struct SamplingTail {
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) policy: Arc<SamplingPolicy>,
    /// The route's rolling latency; `None` for a request no route took.
    pub(crate) latency: Option<Arc<LatencyWindow>>,
    /// The route's group, to turn an endpoint's position into its address.
    pub(crate) upstream: Option<Arc<CompiledUpstream>>,
    /// The client as the proxy sees it (behind a trusted proxy, the real one).
    pub(crate) client_ip: IpAddr,
    pub(crate) head: HeadCapture,
    pub(crate) response: Option<ResponseCapture>,
    pub(crate) answer: Answer,
    pub(crate) attempts: AttemptLog,
    pub(crate) request_body: Option<Arc<BodyStats>>,
    pub(crate) request_content_type: Option<String>,
    pub(crate) response_tee: Option<ResponseTee>,
    pub(crate) response_content_type: Option<String>,
    /// Why there will be no body in the sample, when there will not be.
    pub(crate) body_omitted: Option<&'static str>,
    /// Holds the body memory budget until the sample is handled or dropped.
    pub(crate) reservation: Option<BodyReservation>,
    /// What to ask of the request body while it streams to the upstream, when
    /// the request is forwarded.
    pub(crate) request_observation: Option<BodyObservation>,
}

impl SamplingTail {
    /// Starts gathering what the sampling decision will need, or `None` when
    /// sampling is off for this request. Called before the request is forwarded,
    /// because the request is consumed by then.
    pub(crate) fn begin(
        (sampler, metrics): (&Sampler, &Metrics),
        snapshot: &Arc<Snapshot>,
        source: SamplingSource,
        (headers, method, uri): (&HeaderMap, &Method, &Uri),
        client_ip: IpAddr,
    ) -> Option<Self> {
        if !source.policy.enabled {
            return None;
        }
        let forwarded = source.upstream.is_some();
        let upgrade = headers::is_upgrade_request(headers);
        let plan = plan_bodies(
            &source.policy,
            forwarded,
            upgrade,
            headers,
            sampler.budget(),
        );
        if plan.budget_refused {
            metrics.sample_dropped(SampleDropReason::Budget);
        }
        let extra_headers = &snapshot.sampling_settings.extra_headers;
        Some(Self {
            snapshot: snapshot.clone(),
            policy: source.policy,
            latency: source.latency,
            upstream: source.upstream,
            client_ip,
            head: HeadCapture::take(headers, method, uri, extra_headers),
            response: None,
            answer: Answer::Fixed,
            attempts: AttemptLog::default(),
            request_body: None,
            request_content_type: plan.request_content_type,
            response_tee: None,
            response_content_type: None,
            body_omitted: plan.omitted,
            reservation: plan.reservation,
            request_observation: forwarded.then_some(BodyObservation {
                capture_cap: plan.request_cap,
            }),
        })
    }

    /// The response head is known: keeps its selected headers and, when bodies
    /// are being captured and the response is text, starts keeping its start.
    pub(crate) fn note_response(&mut self, headers: &HeaderMap) {
        self.response = Some(ResponseCapture::take(headers));
        if self.reservation.is_none() {
            return;
        }
        if let Some(content_type) = capturable_content_type(headers) {
            self.response_tee = Some(ResponseTee::new(self.policy.body_response_cap as usize));
            self.response_content_type = Some(content_type);
        }
    }

    /// Decides, and when the request was unusual and the limits allow, hands a
    /// raw sample to the sampler.
    pub(crate) fn finish(self, done: &Finished<'_>) {
        let elapsed_ms = u32::try_from(done.elapsed.as_millis()).unwrap_or(u32::MAX);
        let now = monotonic_ms();
        let answer = self.effective_answer(done.cancelled);
        if let (Some(window), Answer::Upstream, false) = (&self.latency, answer, done.cancelled) {
            window.record(elapsed_ms, now);
        }
        let outcome = Outcome {
            status: done.status,
            elapsed_ms,
            answer,
            attempts: self.attempts,
            bytes_in: self
                .request_body
                .as_ref()
                .map_or(0, |body| body.bytes_read()),
            bytes_out: done.bytes_out,
            client_cancelled: done.cancelled,
        };
        let state = DecisionState {
            latency: self.latency.as_deref().unwrap_or(&NO_WINDOW),
            clients: &self.snapshot.sampling.clients,
        };
        let roll = rand::thread_rng().gen_range(0..1000);
        let triggers = decide(
            &self.policy,
            self.head.flags,
            self.client_ip,
            &outcome,
            &state,
            roll,
            now,
        );
        let Some(primary) = triggers.primary() else {
            return;
        };
        let Some(suppressed) = self.admit(primary, done, now) else {
            return;
        };
        let raw = self.into_raw(done, outcome, triggers, suppressed);
        if done.sampler.try_submit(raw).is_ok() {
            done.metrics.sample_captured(primary.as_str());
        }
    }

    /// A request cancelled while it waited for an upstream is, for the
    /// decision, an upstream one with nothing to say; a fixed answer stays fixed.
    fn effective_answer(&self, cancelled: bool) -> Answer {
        if cancelled && self.latency.is_some() {
            return Answer::Upstream;
        }
        self.answer
    }

    /// The storm guard, then the rate limit. `None` means drop it (and it has
    /// been counted); otherwise how many of this key's candidates were dropped
    /// since the last kept one.
    fn admit(&self, primary: Trigger, done: &Finished<'_>, now: u64) -> Option<Option<Suppressed>> {
        let mut suppressed = None;
        if primary != Trigger::Baseline {
            let key = StormKey::new(
                &done.fields.subsystem,
                &done.fields.route,
                primary,
                failure_key(&self.attempts, primary),
            );
            let guard = &self.snapshot.sampling.storm_guard;
            match guard.admit(
                key,
                self.policy.storm_burst,
                self.policy.storm_interval_secs,
                now,
            ) {
                Admission::Suppress => {
                    done.metrics.sample_dropped(SampleDropReason::StormGuard);
                    return None;
                }
                Admission::Keep { suppressed: 0, .. } => {}
                Admission::Keep {
                    suppressed: count,
                    since_ms,
                } => {
                    suppressed = Some(Suppressed { count, since_ms });
                }
            }
        }
        let limiter = &self.snapshot.sampling.rate_limiter;
        if !limiter.try_acquire(now, self.policy.rate_per_sec, self.policy.rate_burst) {
            done.metrics.sample_dropped(SampleDropReason::RateLimit);
            return None;
        }
        Some(suppressed)
    }

    fn into_raw(
        self,
        done: &Finished<'_>,
        outcome: Outcome,
        triggers: fast_proxy_model::TriggerSet,
        suppressed: Option<Suppressed>,
    ) -> RawSample {
        let request_body = self.request_body.as_ref().and_then(|stats| {
            let content_type = self.request_content_type.clone()?;
            let (bytes, truncated) = stats.captured()?;
            Some(CapturedBody {
                content_type,
                bytes,
                truncated,
            })
        });
        let response_body = self
            .response_tee
            .zip(self.response_content_type)
            .map(|(tee, content_type)| tee.into_body(content_type));
        let (path, query) = split_uri(done.uri);
        let flags = self.head.flags;
        RawSample {
            at_ms: now_unix_ms(),
            request_id: done.fields.request_id.clone(),
            triggers,
            subsystem: done.fields.subsystem.clone(),
            route: done.fields.route.clone(),
            head: RequestHead {
                method: done.method.to_string(),
                path,
                query,
                headers: self.head.into_headers(),
                flags,
                client: done.client,
            },
            outcome,
            attempts: attempt_details(&self.attempts, self.upstream.as_deref()),
            response_headers: self
                .response
                .map(ResponseCapture::into_headers)
                .unwrap_or_default(),
            request_body,
            response_body,
            body_omitted: self.body_omitted,
            suppressed,
            reservation: self.reservation,
        }
    }
}

/// The endpoint and failure kind a connectivity trigger is about: the last
/// failure for a request that failed, the first for one that recovered.
fn failure_key(attempts: &AttemptLog, primary: Trigger) -> Option<(u16, FailureKind)> {
    let mut failures = attempts
        .iter()
        .filter_map(|attempt| attempt.failure.map(|kind| (attempt.endpoint_index, kind)));
    match primary {
        Trigger::UpstreamFailure => failures.last(),
        Trigger::Retried => failures.next(),
        _ => None,
    }
}

fn attempt_details(
    attempts: &AttemptLog,
    upstream: Option<&CompiledUpstream>,
) -> Vec<AttemptDetail> {
    attempts
        .iter()
        .map(|attempt| AttemptDetail {
            endpoint: upstream
                .and_then(|upstream| upstream.endpoints.get(usize::from(attempt.endpoint_index)))
                .map_or_else(String::new, |endpoint| endpoint.addr.to_string()),
            failure: attempt.failure.map(|kind| kind.as_str().to_string()),
            status: attempt.upstream_status,
        })
        .collect()
}

/// The path and the query of the request line as it was received (which may be
/// an absolute URI).
fn split_uri(uri: &str) -> (String, Option<String>) {
    match uri.parse::<Uri>() {
        Ok(parsed) => (
            parsed.path().to_string(),
            parsed.query().map(str::to_string),
        ),
        Err(_) => (uri.to_string(), None),
    }
}

fn now_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use fast_proxy_sampling::{Attempt, FailureKind};

    use super::*;

    fn log(list: &[(u16, Option<FailureKind>)]) -> AttemptLog {
        let mut log = AttemptLog::default();
        for (endpoint_index, failure) in list {
            log.push(Attempt {
                endpoint_index: *endpoint_index,
                failure: *failure,
                upstream_status: failure.is_none().then_some(200),
            });
        }
        log
    }

    fn policy_with_bodies(on: bool) -> SamplingPolicy {
        SamplingPolicy {
            capture_bodies: on,
            body_request_cap: 100,
            body_response_cap: 200,
            ..SamplingPolicy::disabled()
        }
    }

    fn json_post() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", "application/json".parse().unwrap());
        headers.insert("content-length", "12".parse().unwrap());
        headers
    }

    #[test]
    fn bodies_are_off_unless_the_policy_says_otherwise_or_the_request_is_not_forwarded() {
        let budget = BodyBudget::new(10_000);
        let plan = plan_bodies(
            &policy_with_bodies(false),
            true,
            false,
            &json_post(),
            &budget,
        );
        assert_eq!(
            (plan.omitted, plan.request_cap, plan.reservation.is_some()),
            (Some("off"), 0, false)
        );
        let plan = plan_bodies(
            &policy_with_bodies(true),
            false,
            false,
            &json_post(),
            &budget,
        );
        assert_eq!(
            plan.omitted,
            Some("off"),
            "an answer the proxy made itself has no body to capture"
        );
        assert_eq!(budget.used(), 0, "nothing was reserved");
    }

    #[test]
    fn an_upgrade_is_never_captured() {
        let budget = BodyBudget::new(10_000);
        let plan = plan_bodies(&policy_with_bodies(true), true, true, &json_post(), &budget);
        assert_eq!(plan.omitted, Some("upgrade"));
        assert!(plan.reservation.is_none());
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_request_that_fits_the_budget_reserves_both_caps_and_captures_a_text_body() {
        let budget = BodyBudget::new(10_000);
        let plan = plan_bodies(
            &policy_with_bodies(true),
            true,
            false,
            &json_post(),
            &budget,
        );
        assert_eq!(plan.request_cap, 100);
        assert_eq!(
            plan.request_content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(plan.omitted, None);
        assert_eq!(
            budget.used(),
            300,
            "both caps are held until the sample is handled"
        );
        drop(plan);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_spent_budget_skips_the_bodies_and_says_so() {
        let budget = BodyBudget::new(250);
        let plan = plan_bodies(
            &policy_with_bodies(true),
            true,
            false,
            &json_post(),
            &budget,
        );
        assert!(plan.budget_refused);
        assert_eq!(plan.omitted, Some("budget"));
        assert_eq!(plan.request_cap, 0);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_binary_request_body_is_not_captured_but_a_request_without_a_body_says_nothing() {
        let budget = BodyBudget::new(10_000);
        let mut binary = HeaderMap::new();
        binary.insert("content-type", "image/png".parse().unwrap());
        binary.insert("content-length", "9".parse().unwrap());
        let plan = plan_bodies(&policy_with_bodies(true), true, false, &binary, &budget);
        assert_eq!((plan.request_cap, plan.omitted), (0, Some("content_type")));
        let plan = plan_bodies(
            &policy_with_bodies(true),
            true,
            false,
            &HeaderMap::new(),
            &budget,
        );
        assert_eq!(plan.omitted, None, "a GET has no body to omit");
        assert_eq!(plan.request_cap, 0);
    }

    #[test]
    fn the_storm_key_names_the_last_failure_of_a_failed_request_and_the_first_of_a_recovered_one() {
        let attempts = log(&[
            (0, Some(FailureKind::ConnectRefused)),
            (1, Some(FailureKind::ResponseTimeout)),
        ]);
        assert_eq!(
            failure_key(&attempts, Trigger::UpstreamFailure),
            Some((1, FailureKind::ResponseTimeout))
        );
        let recovered = log(&[(0, Some(FailureKind::ConnectRefused)), (1, None)]);
        assert_eq!(
            failure_key(&recovered, Trigger::Retried),
            Some((0, FailureKind::ConnectRefused))
        );
        assert_eq!(failure_key(&attempts, Trigger::Slow), None);
        assert_eq!(
            failure_key(&AttemptLog::default(), Trigger::UpstreamFailure),
            None
        );
    }

    #[test]
    fn the_response_tee_keeps_the_start_and_says_when_there_was_more() {
        let mut tee = ResponseTee::new(8);
        tee.record(b"hello ");
        tee.record(b"world, more");
        let body = tee.into_body("text/plain".into());
        assert_eq!(body.bytes, b"hello wo");
        assert!(body.truncated);
        let mut exact = ResponseTee::new(5);
        exact.record(b"12345");
        let body = exact.into_body("text/plain".into());
        assert_eq!(body.bytes, b"12345");
        assert!(!body.truncated, "a body exactly the cap is not truncated");
    }

    #[test]
    fn the_path_and_query_come_from_a_request_line_or_an_absolute_uri() {
        assert_eq!(
            split_uri("/a/b?x=1&y=2"),
            ("/a/b".into(), Some("x=1&y=2".into()))
        );
        assert_eq!(split_uri("/plain"), ("/plain".into(), None));
        assert_eq!(
            split_uri("http://example.com/p?q=1"),
            ("/p".into(), Some("q=1".into()))
        );
        assert_eq!(
            split_uri("not a uri\u{7f}"),
            ("not a uri\u{7f}".into(), None)
        );
    }
}
