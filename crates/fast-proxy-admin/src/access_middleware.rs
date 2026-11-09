//! Axum middleware that feeds the access log.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use fast_proxy_telemetry::AccessRecord;

use crate::state::SharedState;

/// Middleware recording one access-log line per admin request and
/// counting it toward the request total.
pub async fn record(
    State(state): State<SharedState>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let uri = req.uri().to_string();
    let version = format!("{:?}", req.version());

    let response = next.run(req).await;

    state.requests_total.fetch_add(1, Ordering::Relaxed);
    let record = AccessRecord {
        client,
        method: method.as_str(),
        uri: &uri,
        version: &version,
        status: response.status().as_u16(),
        elapsed: started.elapsed(),
    };
    state.access_log.record(&record).await;

    response
}
