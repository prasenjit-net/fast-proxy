use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use fast_proxy_balancing::{
    monotonic_ms, record_outcome, Candidate, EndpointRuntime, InFlightGuard, Outcome, Transition,
};
use fast_proxy_routing::{build_upstream_uri, CompiledEndpoint};
use fast_proxy_telemetry::{
    EndpointChange, EndpointEvent, EndpointEvents, EndpointLabels, Metrics, RetryReason,
    UpstreamErrorKind, UpstreamOutcome,
};
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioTimer};

use crate::connections::ConnectionContext;
use crate::headers::{self, ClientInfo};
use crate::protocol_switch::{into_client_response, switch_protocols};
use crate::replay_body::{BodyOwner, ReplayBody};
use crate::retry_policy::{retry_reason, AttemptFailure};
use crate::selection::select_endpoint;
use crate::trusted_proxies::TrustedProxies;
use crate::upstream_errors::{kind_for, outcome_of_error, outcome_of_status};
use crate::{FixedResponse, Forward, Forwarded, ResponseBody, Target};

type HttpClient = Client<HttpConnector, ReplayBody>;

/// How long an unused upstream connection stays pooled.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Forwards routed requests to their upstream over pooled HTTP/1.1
/// connections, streaming both bodies without buffering, and retries
/// failed attempts on another endpoint within the route's retry policy and
/// the group's retry budget.
///
/// The connect timeout belongs to the connector, so one client (and pool)
/// is kept per distinct connect timeout; the response timeout is applied
/// per attempt.
pub struct UpstreamClient {
    trusted_proxies: TrustedProxies,
    metrics: Arc<Metrics>,
    events: EndpointEvents,
    clients: Mutex<HashMap<Duration, HttpClient>>,
}

impl UpstreamClient {
    pub fn new(
        trusted_proxies: TrustedProxies,
        metrics: Arc<Metrics>,
        events: EndpointEvents,
    ) -> Self {
        Self {
            trusted_proxies,
            metrics,
            events,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client_for(&self, connect_timeout: Duration) -> HttpClient {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        clients
            .entry(connect_timeout)
            .or_insert_with(|| build_client(connect_timeout))
            .clone()
    }

    /// Counts an upstream failure and builds the matching 502/504 response.
    fn failed(&self, target: &Target<'_>, kind: UpstreamErrorKind) -> Response<ResponseBody> {
        self.metrics
            .upstream_error(target.subsystem.name.as_str(), kind);
        let fixed = match kind {
            UpstreamErrorKind::Timeout => FixedResponse::GatewayTimeout,
            UpstreamErrorKind::Connect | UpstreamErrorKind::Other => FixedResponse::BadGateway,
        };
        fixed.render(target.request_id, target.wants_json)
    }
}

/// Clears a half-open trial claim when an attempt ends without a verdict
/// (the client left mid-request), so the endpoint is not stuck waiting.
struct TrialRelease<'a>(&'a EndpointRuntime);

impl Drop for TrialRelease<'_> {
    fn drop(&mut self) {
        self.0.release_trial();
    }
}

/// The endpoint one attempt goes to.
#[derive(Clone, Copy)]
struct Chosen<'a> {
    index: usize,
    endpoint: &'a CompiledEndpoint,
    candidate: &'a Candidate,
}

/// How one attempt ended.
enum AttemptEnd {
    Response(Response<Incoming>),
    Failed(UpstreamErrorKind),
}

impl UpstreamClient {
    /// Feeds one finished attempt to the metrics and to passive health, and
    /// logs the transition it caused (an ejection or a recovery).
    fn report(&self, target: &Target<'_>, chosen: Chosen<'_>, outcome: UpstreamOutcome) {
        let upstream = &target.route.upstream;
        let addr = chosen.endpoint.addr.to_string();
        let labels = EndpointLabels {
            subsystem: target.subsystem.name.as_str(),
            upstream: &upstream.name,
            endpoint: &addr,
        };
        self.metrics.upstream_request(labels, outcome);
        let verdict = match outcome {
            UpstreamOutcome::Success => Outcome::Success,
            _ => Outcome::Failure,
        };
        let transition = record_outcome(
            &upstream.candidates,
            chosen.index,
            verdict,
            &upstream.policy.passive,
            monotonic_ms(),
        );
        let announce = |change| {
            self.events.publish(EndpointEvent {
                subsystem: labels.subsystem.to_string(),
                upstream: labels.upstream.to_string(),
                endpoint: addr.clone(),
                change,
            });
        };
        match transition {
            Some(Transition::Ejected { until_ms, times }) => {
                announce(EndpointChange::Ejected { until_ms, times });
                self.metrics.endpoint_ejected(labels);
                tracing::warn!(
                    upstream = %upstream.name, endpoint = %addr, times, until_ms,
                    "endpoint ejected after repeated failures"
                );
            }
            Some(Transition::Recovered) => {
                announce(EndpointChange::Recovered);
                tracing::info!(upstream = %upstream.name, endpoint = %addr, "endpoint recovered");
            }
            None => {}
        }
    }

    /// Sends one attempt to `chosen` and reports how it went.
    async fn attempt(
        &self,
        client: &HttpClient,
        request: Request<ReplayBody>,
        target: &Target<'_>,
        chosen: Chosen<'_>,
    ) -> AttemptEnd {
        let _trial = TrialRelease(&chosen.candidate.runtime);
        let sent = Instant::now();
        let route = target.route;
        let outcome = tokio::time::timeout(route.response_timeout, client.request(request)).await;
        match outcome {
            Err(_elapsed) => {
                tracing::warn!(upstream = %chosen.endpoint.addr, "upstream response timed out");
                self.report(target, chosen, UpstreamOutcome::Timeout);
                AttemptEnd::Failed(UpstreamErrorKind::Timeout)
            }
            Ok(Err(err)) => {
                tracing::warn!(upstream = %chosen.endpoint.addr, "upstream request failed: {err}");
                let kind = kind_for(err.is_connect(), &err);
                self.report(target, chosen, outcome_of_error(kind));
                AttemptEnd::Failed(kind)
            }
            Ok(Ok(response)) => {
                let micros = u64::try_from(sent.elapsed().as_micros()).unwrap_or(u64::MAX);
                chosen
                    .candidate
                    .runtime
                    .record_latency(micros, monotonic_ms());
                self.report(target, chosen, outcome_of_status(response.status()));
                AttemptEnd::Response(response)
            }
        }
    }

    /// Decides whether to try again after `failed`, and where: the retry
    /// rules, then the group's budget, then another endpoint.
    fn next_endpoint<'a>(
        &self,
        target: &Target<'a>,
        head: &http::request::Parts,
        failed: &FailedAttempt<'_>,
    ) -> Option<Chosen<'a>> {
        let reason: RetryReason = retry_reason(
            failed.policy,
            &head.method,
            failed.failure,
            failed.replayable,
            failed.attempts_made,
        )?;
        let upstream = &target.route.upstream;
        let subsystem = target.subsystem.name.as_str();
        if !upstream.retry_budget.try_acquire_retry(monotonic_ms()) {
            self.metrics
                .retry_budget_exhausted(subsystem, &upstream.name);
            return None;
        }
        let selection = select_endpoint(
            upstream,
            &head.headers,
            target.client_addr.ip(),
            Some(failed.index),
        )?;
        if selection.panic {
            self.metrics.upstream_panic(subsystem, &upstream.name);
        }
        self.metrics
            .upstream_retry(subsystem, &upstream.name, reason);
        Some(Chosen {
            index: selection.index,
            endpoint: &upstream.endpoints[selection.index],
            candidate: &upstream.candidates[selection.index],
        })
    }
}

/// What the retry rules need to know about a failed attempt.
struct FailedAttempt<'a> {
    index: usize,
    failure: AttemptFailure,
    /// The whole request body can still be sent again.
    replayable: bool,
    attempts_made: u32,
    policy: &'a fast_proxy_model::RetryPolicy,
}

fn build_client(connect_timeout: Duration) -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(connect_timeout));
    connector.set_nodelay(true);
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_timer(TokioTimer::new())
        // Retrying is done by the attempt loop, under the route's policy and
        // the group's budget; a silent retry here could replay a request.
        .retry_canceled_requests(false)
        .build(connector)
}

/// The request for one attempt: the shared head with `Host` and URI set for
/// the chosen endpoint, and a fresh view of the body.
fn build_attempt(
    head: &http::request::Parts,
    target: &Target<'_>,
    chosen: Chosen<'_>,
    body: ReplayBody,
) -> Option<Request<ReplayBody>> {
    let uri = build_upstream_uri(
        target.route,
        &chosen.endpoint.addr,
        &head.uri,
        target.consumed_prefix_len,
    )?;
    let mut request = Request::new(body);
    let mut headers = head.headers.clone();
    headers::apply_host(
        &mut headers,
        target.route.preserve_host,
        &chosen.endpoint.addr.to_string(),
    );
    *request.method_mut() = head.method.clone();
    *request.version_mut() = head.version;
    *request.headers_mut() = headers;
    *request.uri_mut() = uri;
    Some(request)
}

impl Forward for UpstreamClient {
    async fn forward<'a>(
        &'a self,
        mut request: Request<Incoming>,
        target: Target<'a>,
    ) -> Forwarded {
        let route = target.route;
        let upstream = &route.upstream;

        // A protocol switch needs the client's side of the socket, which is
        // only available before the request is handed to the upstream client.
        let upgrading = headers::is_upgrade_request(request.headers());
        let context = request.extensions().get::<ConnectionContext>().cloned();
        let client_upgrade = upgrading.then(|| hyper::upgrade::on(&mut request));

        let (mut head, body) = request.into_parts();
        if upgrading {
            headers::strip_hop_by_hop_keeping_upgrade(&mut head.headers);
        } else {
            headers::strip_hop_by_hop(&mut head.headers);
        }
        headers::apply_forwarded(
            &mut head.headers,
            &ClientInfo {
                ip: target.client_addr.ip(),
                trusted: self.trusted_proxies.contains(target.client_addr.ip()),
                scheme: "http",
            },
        );
        headers::apply_request_id(&mut head.headers, target.request_id);

        let owner = BodyOwner::new(body);
        let client = self.client_for(route.connect_timeout);
        // A tunnel cannot be replayed, so an upgrade gets exactly one attempt.
        let policy = if upgrading {
            fast_proxy_model::RetryPolicy {
                max_attempts: 1,
                ..route.retry
            }
        } else {
            route.retry
        };
        upstream.retry_budget.record_request(monotonic_ms());

        let mut chosen = Chosen {
            index: target.endpoint_index,
            endpoint: target.endpoint,
            candidate: target.candidate,
        };
        let mut attempts = 0;
        loop {
            attempts += 1;
            let in_flight = chosen.candidate.runtime.track();
            let Some(attempt) = build_attempt(&head, &target, chosen, owner.attempt()) else {
                tracing::warn!(route = route.id.0, "cannot build an upstream URI");
                let response = self.failed(&target, UpstreamErrorKind::Other);
                return finished(response, chosen.index, attempts - 1, None);
            };
            let failure = match self.attempt(&client, attempt, &target, chosen).await {
                AttemptEnd::Response(response) if !is_bad_status(response.status()) => {
                    let response = match client_upgrade {
                        Some(client_upgrade)
                            if response.status() == StatusCode::SWITCHING_PROTOCOLS =>
                        {
                            let tunnel = chosen.candidate.runtime.track();
                            switch_protocols(response, client_upgrade, context, tunnel)
                        }
                        _ => into_client_response(response),
                    };
                    return finished(response, chosen.index, attempts, Some(in_flight));
                }
                AttemptEnd::Response(response) => {
                    let failed = FailedAttempt {
                        index: chosen.index,
                        failure: AttemptFailure::BadStatus,
                        replayable: owner.is_replayable(),
                        attempts_made: attempts,
                        policy: &policy,
                    };
                    match self.next_endpoint(&target, &head, &failed) {
                        Some(next) => {
                            chosen = next;
                            continue;
                        }
                        None => {
                            let response = into_client_response(response);
                            return finished(response, chosen.index, attempts, Some(in_flight));
                        }
                    }
                }
                AttemptEnd::Failed(kind) => kind,
            };
            let attempt_failure = match failure {
                UpstreamErrorKind::Connect => AttemptFailure::Connect,
                UpstreamErrorKind::Timeout => AttemptFailure::Timeout,
                UpstreamErrorKind::Other => AttemptFailure::Reset,
            };
            let failed = FailedAttempt {
                index: chosen.index,
                failure: attempt_failure,
                replayable: owner.is_replayable(),
                attempts_made: attempts,
                policy: &policy,
            };
            match self.next_endpoint(&target, &head, &failed) {
                Some(next) => chosen = next,
                None => {
                    let response = self.failed(&target, failure);
                    return finished(response, chosen.index, attempts, None);
                }
            }
        }
    }
}

fn is_bad_status(status: StatusCode) -> bool {
    outcome_of_status(status) == UpstreamOutcome::BadStatus
}

fn finished(
    response: Response<ResponseBody>,
    endpoint_index: usize,
    attempts: u32,
    in_flight: Option<InFlightGuard>,
) -> Forwarded {
    Forwarded {
        response,
        endpoint_index,
        attempts,
        in_flight,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_client_is_kept_per_distinct_connect_timeout() {
        let client = UpstreamClient::new(
            TrustedProxies::default(),
            Arc::new(Metrics::new()),
            EndpointEvents::new(),
        );
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(2));
        assert_eq!(client.clients.lock().unwrap().len(), 2);
    }
}
