use std::future::Future;
use std::net::SocketAddr;

use std::sync::Arc;

use bytes::Bytes;
use fast_proxy_balancing::{Candidate, InFlightGuard};
use fast_proxy_routing::{CompiledEndpoint, CompiledRoute, CompiledSubsystem};
use fast_proxy_sampling::{Attempt, AttemptLog};
use http::{Request, Response};
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;

use crate::replay_body::{BodyObservation, BodyStats};

/// Every response body the data plane produces, fixed or streamed.
pub type ResponseBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// What the pipeline resolved for a request, borrowed from the snapshot that
/// stays alive for the whole request.
pub struct Target<'snapshot> {
    pub subsystem: &'snapshot CompiledSubsystem,
    pub route: &'snapshot CompiledRoute,
    /// The endpoint of the route's upstream chosen for this request.
    pub endpoint: &'snapshot CompiledEndpoint,
    /// The chosen endpoint's position in the upstream's endpoint list.
    pub endpoint_index: usize,
    /// The chosen endpoint's live state, for latency samples and tunnels.
    pub candidate: &'snapshot Candidate,
    /// How many bytes of the request path the route's prefix consumed; the
    /// rest of the path is `request.uri().path()[consumed_prefix_len..]`.
    pub consumed_prefix_len: usize,
    /// The request's id, already validated or generated.
    pub request_id: &'snapshot str,
    /// The client's socket address, as seen by the listener.
    pub client_addr: SocketAddr,
    /// Whether the client asked for JSON, so error responses match.
    pub wants_json: bool,
    /// Whether to count (and perhaps capture) the request body as it streams to
    /// the upstream, for the sampler.
    pub request_body: Option<BodyObservation>,
}

/// What forwarding a request produced: the response and what it took.
pub struct Forwarded {
    pub response: Response<ResponseBody>,
    /// The endpoint that produced the response (the last one tried).
    pub endpoint_index: usize,
    /// How many upstream attempts were made.
    pub attempts: u32,
    /// Keeps the answering endpoint's in-flight count up until the response
    /// body has finished.
    pub in_flight: Option<InFlightGuard>,
    /// How each attempt went, for the sampler.
    pub attempt_log: AttemptLog,
    /// The response is the proxy's own 502 or 504 because the last attempt
    /// failed (an upstream's own error answer is not).
    pub proxy_failed: bool,
    /// What was seen of the request body, when it was observed and there is one.
    pub request_body: Option<Arc<BodyStats>>,
}

impl Forwarded {
    /// A response that took exactly one attempt on `endpoint_index`.
    pub fn single(response: Response<ResponseBody>, endpoint_index: usize) -> Self {
        let mut attempt_log = AttemptLog::default();
        attempt_log.push(Attempt {
            endpoint_index: u16::try_from(endpoint_index).unwrap_or(u16::MAX),
            failure: None,
            upstream_status: Some(response.status().as_u16()),
        });
        Self {
            response,
            endpoint_index,
            attempts: 1,
            in_flight: None,
            attempt_log,
            proxy_failed: false,
            request_body: None,
        }
    }
}

/// Sends a routed request to its upstream and returns the response. It
/// reports upstream failures as 502/504 responses, never as errors.
pub trait Forward: Send + Sync + 'static {
    fn forward<'a>(
        &'a self,
        request: Request<Incoming>,
        target: Target<'a>,
    ) -> impl Future<Output = Forwarded> + Send + 'a;
}
