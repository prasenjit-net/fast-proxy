//! Who is behind a write request. Every handler that changes anything takes a
//! [`Change`] and hands its context to `ConfigService`, which records it on the
//! revision and in the audit log.

use std::convert::Infallible;
use std::net::SocketAddr;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use fast_proxy_config_service::ChangeContext;

/// The context of the current request. Until admin authentication lands every
/// request is recorded as `anonymous`, with the peer's address.
pub struct Change(pub ChangeContext);

impl<S: Send + Sync> FromRequestParts<S> for Change {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let source = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map_or_else(|| "unknown".to_string(), |peer| peer.0.ip().to_string());
        Ok(Self(ChangeContext::unauthenticated(source, "")))
    }
}
