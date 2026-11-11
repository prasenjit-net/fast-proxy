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

/// Who made a request, attached to its response so the access log and the
/// audit log can name the actor (never the token).
#[derive(Debug, Clone)]
pub struct Caller {
    pub name: String,
    pub role: &'static str,
}

impl Caller {
    fn of(principal: &Principal) -> Self {
        Self {
            name: principal.name.clone(),
            role: principal.role.as_str(),
        }
    }
}

/// Why a request was not authenticated, attached to its 401 for the audit log.
#[derive(Debug, Clone)]
pub struct AuthFailure {
    pub reason: String,
    /// This failure was the one that put the source address under the throttle.
    pub throttled: bool,
}

/// A request the guard turned away: the answer, who it was if known, and why
/// authentication failed if it did.
struct Refusal {
    error: AppError,
    caller: Option<Principal>,
    failure: Option<AuthFailure>,
}

impl Refusal {
    fn of(error: AppError) -> Self {
        Self {
            error,
            caller: None,
            failure: None,
        }
    }

    fn forbidden(error: AppError, caller: Principal) -> Self {
        Self {
            error,
            caller: Some(caller),
            failure: None,
        }
    }

    fn into_response(self) -> Response {
        let mut response = self.error.into_response();
        if let Some(caller) = &self.caller {
            response.extensions_mut().insert(Caller::of(caller));
        }
        if let Some(failure) = self.failure {
            response.extensions_mut().insert(failure);
        }
        response
    }
}

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
    let to_note = principal.as_ref().and_then(|principal| {
        token_use_to_note(&state, access, &request, principal)
            .map(|context| (context, principal.token_id))
    });
    if let Some((context, id)) = to_note {
        state.config.record_token_use(&context, id).await;
    }
    let caller = principal.as_ref().map(Caller::of);
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
    }
    let mut response = next.run(request).await;
    if let Some(caller) = caller {
        response.extensions_mut().insert(caller);
    }
    response
}

/// Notes that a token authenticated a request, at most once a minute per token;
/// its first use ever is audited. Tickets and authentication-off requests do not
/// count: the ticket's token already did.
fn token_use_to_note(
    state: &SharedState,
    access: Access,
    request: &Request,
    principal: &Principal,
) -> Option<fast_proxy_config_service::ChangeContext> {
    let by_token = state.security.auth_required && matches!(access, Access::Needs(_));
    if !by_token
        || !state
            .security
            .token_use_due(principal.token_id, crate::clock::now_ms())
    {
        return None;
    }
    let request_id = request
        .extensions()
        .get::<crate::request_id::RequestId>()
        .map_or_else(String::new, |id| id.0.clone());
    Some(fast_proxy_config_service::ChangeContext::token(
        principal.name.clone(),
        principal.role.as_str(),
        peer_ip(request).to_string(),
        request_id,
    ))
}

/// `Ok(None)` for an open route, `Ok(Some(_))` for an authorized caller.
fn authorize(
    state: &SharedState,
    access: Access,
    request: &Request,
) -> Result<Option<Principal>, Refusal> {
    let (Access::Needs(action) | Access::Ticket(action)) = access else {
        return Ok(None);
    };
    let principal = if state.security.auth_required {
        authenticate(state, access, request)?
    } else {
        anonymous()
    };
    if !principal.role.permits(action) {
        return Err(Refusal::forbidden(
            AppError::Forbidden("this token's role does not allow that".into()),
            principal,
        ));
    }
    if action == Action::Keys {
        if let Err(error) = state.security.require_protected_channel() {
            return Err(Refusal::forbidden(error, principal));
        }
    }
    Ok(Some(principal))
}

fn authenticate(
    state: &SharedState,
    access: Access,
    request: &Request,
) -> Result<Principal, Refusal> {
    let security = &state.security;
    let source = peer_ip(request);
    let now = crate::clock::now_ms();
    security.throttle.check(source, now).map_err(|throttled| {
        Refusal::of(AppError::TooManyRequests {
            retry_after_secs: throttled.retry_after_secs,
        })
    })?;
    let outcome = match access {
        Access::Ticket(_) => ticket(request)
            .ok_or("no ticket presented".to_string())
            .and_then(|ticket| {
                security
                    .tickets
                    .redeem(ticket, now)
                    .ok_or("unknown, used or expired ticket".to_string())
            }),
        _ => bearer(request)
            .ok_or("no token presented".to_string())
            .and_then(|token| {
                security
                    .tokens
                    .authenticate(token, now)
                    .map_err(|err| err.to_string())
            }),
    };
    outcome.map_err(|reason| {
        let throttled = security.throttle.record_failure(source, now);
        Refusal {
            error: AppError::Unauthorized("a valid admin token is required".into()),
            caller: None,
            failure: Some(AuthFailure { reason, throttled }),
        }
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
