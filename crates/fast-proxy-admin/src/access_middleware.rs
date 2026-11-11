//! Axum middleware that feeds the access log and stamps every response with
//! the request's id.

use std::net::SocketAddr;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;
use fast_proxy_telemetry::{AccessRecord, Plane};

use crate::guard::{AuthFailure, Caller};
use crate::refusal_audit::{self, Finished};
use crate::request_id::RequestId;
use crate::state::SharedState;

/// The WebSocket's single-use ticket travels in its query, so the log keeps
/// only the path.
const REDACTED_QUERY_PATH: &str = "/ws";

/// Middleware recording one access-log line per admin request, naming the
/// actor (never the token) once the guard has authenticated it. It also gives
/// the request an id, for the audit log and the `X-Request-Id` header.
pub async fn record(
    State(state): State<SharedState>,
    ConnectInfo(client): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let uri = loggable_uri(&req);
    let version = format!("{:?}", req.version());
    let request_id = RequestId::next();
    req.extensions_mut().insert(request_id.clone());
    let path = req.uri().path().to_string();
    let host = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let mut response = next.run(req).await;
    if let Ok(value) = HeaderValue::from_str(&request_id.0) {
        response.headers_mut().insert("x-request-id", value);
    }

    let caller = response.extensions().get::<Caller>().cloned();
    let failure = response.extensions().get::<AuthFailure>().cloned();
    let actor = caller.as_ref().map(|caller| caller.name.clone());
    let record = AccessRecord {
        plane: Plane::Admin,
        client,
        method: method.as_str(),
        uri: &uri,
        version: &version,
        status: response.status().as_u16(),
        elapsed: started.elapsed(),
        data_plane: None,
        actor: actor.as_deref(),
    };
    state.access_log.record(&record);

    refusal_audit::record(
        &state,
        &Finished {
            method: &method,
            path: &path,
            host: host.as_deref(),
            status: response.status(),
            source: client.ip().to_string(),
            request_id: &request_id.0,
            caller: caller.as_ref(),
            failure: failure.as_ref(),
        },
    )
    .await;

    response
}

/// The request target for the log, without the query of `/ws`.
fn loggable_uri(req: &Request) -> String {
    if req.uri().path() == REDACTED_QUERY_PATH {
        return REDACTED_QUERY_PATH.to_string();
    }
    req.uri().to_string()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::*;

    fn request(uri: &str) -> Request {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[test]
    fn the_websocket_ticket_never_reaches_the_log() {
        assert_eq!(loggable_uri(&request("/ws?ticket=secret")), "/ws");
        assert_eq!(loggable_uri(&request("/ws")), "/ws");
    }

    #[test]
    fn other_requests_are_logged_as_sent() {
        assert_eq!(
            loggable_uri(&request("/api/revisions?limit=5")),
            "/api/revisions?limit=5"
        );
    }
}
