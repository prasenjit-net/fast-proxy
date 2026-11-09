use std::future::Future;

use bytes::Bytes;
use fast_proxy_routing::{CompiledRoute, CompiledSubsystem};
use http::{Request, Response};
use http_body_util::combinators::BoxBody;
use hyper::body::Incoming;

/// Every response body the data plane produces, fixed or streamed.
pub type ResponseBody = BoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>;

/// What the pipeline resolved for a request, borrowed from the snapshot that
/// stays alive for the whole request.
pub struct Target<'snapshot> {
    pub subsystem: &'snapshot CompiledSubsystem,
    pub route: &'snapshot CompiledRoute,
    /// How many bytes of the request path the route's prefix consumed; the
    /// rest of the path is `request.uri().path()[consumed_prefix_len..]`.
    pub consumed_prefix_len: usize,
    /// The request's id, already validated or generated.
    pub request_id: &'snapshot str,
}

/// Sends a routed request to its upstream and returns the response. It
/// reports upstream failures as 502/504 responses, never as errors.
pub trait Forward: Send + Sync + 'static {
    fn forward<'a>(
        &'a self,
        request: Request<Incoming>,
        target: Target<'a>,
    ) -> impl Future<Output = Response<ResponseBody>> + Send + 'a;
}
