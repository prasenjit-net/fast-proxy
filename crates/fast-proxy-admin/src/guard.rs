//! The per-route guard: throttle, authenticate, authorize. Every route is
//! registered with an [`Access`] (see `route_table`), so no route can be added
//! without saying what it needs.

use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, Request};
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use fast_proxy_auth::Principal;
use fast_proxy_model::{Action, Role, TokenId};

use crate::state::SharedState;
use crate::AppError;

/// What a route needs from the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Nothing: the health check.
    Open,
    /// A bearer token whose role permits the action.
    Needs(Action),
    /// A single-use WebSocket ticket (in the query) whose role permits the action.
    Ticket(Action),
}

/// The name of the caller, attached to the response so the access log can name
/// who made the request (never the token).
#[derive(Debug, Clone)]
pub struct ActorName(pub String);

/// The caller when authentication is off.
fn anonymous() -> Principal {
    Principal {
        token_id: TokenId(0),
        name: "anonymous".into(),
        role: Role::Admin,
        expires_at_ms: None,
    }
}

/// Middleware body for one route: lets the request through to its handler or
/// answers it with the refusal.
pub async fn guard(
    state: SharedState,
    access: Access,
    mut request: Request,
    next: Next,
) -> Response {
    let principal = match authorize(&state, access, &request) {
        Ok(principal) => principal,
        Err(refusal) => return refusal.into_response(),
    };
    let actor = principal.as_ref().map(|p| ActorName(p.name.clone()));
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
    }
    let mut response = next.run(request).await;
    if let Some(actor) = actor {
        response.extensions_mut().insert(actor);
    }
    response
}

/// `Ok(None)` for an open route, `Ok(Some(_))` for an authorized caller.
fn authorize(
    state: &SharedState,
    access: Access,
    request: &Request,
) -> Result<Option<Principal>, AppError> {
    let (Access::Needs(action) | Access::Ticket(action)) = access else {
        return Ok(None);
    };
    let principal = if state.security.auth_required {
        authenticate(state, access, request)?
    } else {
        anonymous()
    };
    if !principal.role.permits(action) {
        return Err(AppError::Forbidden(
            "this token's role does not allow that".into(),
        ));
    }
    if action == Action::Keys {
        state.security.require_protected_channel()?;
    }
    Ok(Some(principal))
}

fn authenticate(
    state: &SharedState,
    access: Access,
    request: &Request,
) -> Result<Principal, AppError> {
    let security = &state.security;
    let source = peer_ip(request);
    let now = crate::clock::now_ms();
    security
        .throttle
        .check(source, now)
        .map_err(|throttled| AppError::TooManyRequests {
            retry_after_secs: throttled.retry_after_secs,
        })?;
    let authenticated = match access {
        Access::Ticket(_) => {
            ticket(request).and_then(|ticket| security.tickets.redeem(ticket, now))
        }
        _ => bearer(request).and_then(|token| security.tokens.authenticate(token, now).ok()),
    };
    authenticated.ok_or_else(|| {
        security.throttle.record_failure(source, now);
        AppError::Unauthorized("a valid admin token is required".into())
    })
}

fn peer_ip(request: &Request) -> IpAddr {
    peer_ip_of(request.extensions())
}

/// The connecting peer's address, or the unspecified address when unknown.
pub(crate) fn peer_ip_of(extensions: &axum::http::Extensions) -> IpAddr {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map_or(IpAddr::from([0, 0, 0, 0]), |peer| peer.0.ip())
}

/// The token in `Authorization: Bearer <token>`.
fn bearer(request: &Request) -> Option<&str> {
    let value = request.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

/// The `ticket` query parameter (tickets are URL-safe, so nothing to decode).
fn ticket(request: &Request) -> Option<&str> {
    request
        .uri()
        .query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("ticket="))
        .filter(|ticket| !ticket.is_empty())
}

#[cfg(test)]
mod tests {
    use axum::body::Body;

    use super::*;

    fn request(uri: &str, authorization: Option<&str>) -> Request {
        let mut builder = Request::builder().uri(uri);
        if let Some(value) = authorization {
            builder = builder.header(AUTHORIZATION, value);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[test]
    fn a_bearer_token_is_read_case_insensitively() {
        for header in ["Bearer abc", "bearer abc", "BEARER abc", "Bearer  abc "] {
            assert_eq!(bearer(&request("/", Some(header))), Some("abc"), "{header}");
        }
    }

    #[test]
    fn other_schemes_and_empty_tokens_are_not_bearer_tokens() {
        for header in ["Basic abc", "Bearer", "Bearer ", "abc", ""] {
            assert_eq!(bearer(&request("/", Some(header))), None, "{header:?}");
        }
        assert_eq!(bearer(&request("/", None)), None);
    }

    #[test]
    fn the_ticket_is_found_among_other_parameters() {
        assert_eq!(ticket(&request("/ws?ticket=abc", None)), Some("abc"));
        assert_eq!(
            ticket(&request("/ws?x=1&ticket=abc&y=2", None)),
            Some("abc")
        );
        assert_eq!(ticket(&request("/ws?ticket=", None)), None);
        assert_eq!(ticket(&request("/ws?xticket=abc", None)), None);
        assert_eq!(ticket(&request("/ws", None)), None);
    }
}
