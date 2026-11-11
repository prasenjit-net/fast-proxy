//! Who is behind a write request. Every handler that changes anything takes a
//! [`Change`] and hands its context to `ConfigService`, which records it on the
//! revision and in the audit log.

use std::convert::Infallible;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fast_proxy_auth::Principal;
use fast_proxy_config_service::ChangeContext;

use crate::guard::peer_ip_of;
use crate::request_id::RequestId;

/// The context of the current request: the authenticated token's name and
/// role (`anonymous` when authentication is off), where it came from, and its
/// request id.
pub struct Change(pub ChangeContext);

impl<S: Send + Sync> FromRequestParts<S> for Change {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let source = peer_ip_of(&parts.extensions).to_string();
        let request_id = parts
            .extensions
            .get::<RequestId>()
            .map_or_else(String::new, |id| id.0.clone());
        let context = match parts.extensions.get::<Principal>() {
            Some(principal) => ChangeContext::token(
                principal.name.clone(),
                principal.role.as_str(),
                source,
                request_id,
            ),
            None => ChangeContext::unauthenticated(source, request_id),
        };
        Ok(Self(context))
    }
}
