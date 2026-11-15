//! Answering ACME HTTP-01 challenges (ADR 0025). The answer comes from the
//! snapshot, ahead of the HTTPS redirect, the 421 rule and maintenance, so a
//! proxy that redirects everything, or is in maintenance, still renews its
//! certificates. Only a `GET` or `HEAD` for a live token issued for that host is
//! answered; anything else on the path is routed as usual.

use std::sync::Arc;

use fast_proxy_routing::Snapshot;
use http::{header, HeaderValue, Method, Request, Response, StatusCode};

use crate::request_id::{header_value, REQUEST_ID};
use crate::responses::boxed;
use crate::ResponseBody;

/// The key authorization to answer `request` with, if it is a challenge this
/// proxy holds for the request's host.
pub(crate) fn answer_for<B>(
    snapshot: &Snapshot,
    request: &Request<B>,
    host_header: &str,
) -> Option<Arc<str>> {
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return None;
    }
    snapshot
        .acme_challenges
        .answer(host_header, request.uri().path())
}

/// The `200 text/plain` answer: the key authorization, never cached.
pub(crate) fn render(key_authorization: &str, request_id: &str) -> Response<ResponseBody> {
    let mut response = Response::new(boxed(key_authorization.to_string()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(REQUEST_ID, header_value(request_id));
    response
}
