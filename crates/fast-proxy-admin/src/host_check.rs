//! Refuses requests whose `Host` is not one this admin plane answers to.

use axum::extract::{Request, State};
use axum::http::header::HOST;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::state::SharedState;
use crate::AppError;

/// Answers 421 unless the request's `Host` (or the authority of an HTTP/2
/// request) is on the allow-list. A request with no host at all is refused too.
pub async fn check(State(state): State<SharedState>, request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    match host {
        Some(host) if state.security.allows_host(host) => next.run(request).await,
        _ => AppError::Misdirected("this Host is not served by the admin plane".into())
            .into_response(),
    }
}
