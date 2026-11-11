//! The audit entries of requests that changed nothing: authentication
//! failures, refusals, a foreign `Host`, and writes the server rejected. (A
//! committed change writes its own entry in its own transaction, so these
//! never duplicate one.) Entries hold the method, the path without its query
//! (the `/ws` ticket lives in the query), a status and a short reason, never a
//! body, a token or a ticket.

use axum::http::{Method, StatusCode};
use fast_proxy_config_service::{AuditEvent, ChangeContext};
use fast_proxy_store::repository::audit::AuditOutcome;
use serde_json::json;

use crate::guard::{AuthFailure, Caller};
use crate::state::SharedState;

/// Longest `Host` value kept in an entry.
const MAX_HOST_CHARS: usize = 200;

/// What the access middleware knows about a finished request.
pub(crate) struct Finished<'a> {
    pub method: &'a Method,
    pub path: &'a str,
    pub host: Option<&'a str>,
    pub status: StatusCode,
    pub source: String,
    pub request_id: &'a str,
    pub caller: Option<&'a Caller>,
    pub failure: Option<&'a AuthFailure>,
}

/// Records the request if it is one of the kinds worth an entry.
pub(crate) async fn record(state: &SharedState, request: &Finished<'_>) {
    let Some(entry) = entry_for(request) else {
        return;
    };
    if !entry.authenticated
        && !state
            .security
            .unauthenticated_audit_allowed(crate::clock::now_ms())
    {
        return;
    }
    let (actor, role) = request
        .caller
        .map_or(("anonymous".to_string(), "none"), |caller| {
            (caller.name.clone(), caller.role)
        });
    let context = ChangeContext {
        actor,
        role: role.to_string(),
        source: request.source.clone(),
        request_id: request.request_id.to_string(),
    };
    state.config.record_audit(&context, entry.event).await;
}

/// An event, and whether the caller was authenticated (unauthenticated events
/// are rate-limited for the whole process so a flood cannot grow the log).
struct Entry {
    event: AuditEvent,
    authenticated: bool,
}

fn entry_for(request: &Finished<'_>) -> Option<Entry> {
    let status = request.status.as_u16();
    let common = json!({
        "status": status,
        "method": request.method.as_str(),
    });
    let with = |extra: serde_json::Value| {
        let mut detail = common.clone();
        if let (Some(base), Some(more)) = (detail.as_object_mut(), extra.as_object()) {
            base.extend(more.clone());
        }
        detail
    };
    match request.status {
        StatusCode::UNAUTHORIZED => Some(Entry {
            authenticated: false,
            event: AuditEvent {
                action: "auth.failed",
                target: request.path.to_string(),
                outcome: AuditOutcome::Denied,
                detail: with(json!({
                    "reason": request.failure.map(|f| f.reason.as_str()),
                    "throttled": request.failure.is_some_and(|f| f.throttled),
                })),
            },
        }),
        StatusCode::MISDIRECTED_REQUEST => Some(Entry {
            authenticated: false,
            event: AuditEvent {
                action: "host.refused",
                target: request.path.to_string(),
                outcome: AuditOutcome::Denied,
                detail: with(json!({
                    "host": request
                        .host
                        .map(|host| host.chars().take(MAX_HOST_CHARS).collect::<String>()),
                })),
            },
        }),
        StatusCode::FORBIDDEN => Some(Entry {
            authenticated: request.caller.is_some(),
            event: AuditEvent {
                action: "access.denied",
                target: request.path.to_string(),
                outcome: AuditOutcome::Denied,
                detail: with(json!({})),
            },
        }),
        StatusCode::BAD_REQUEST | StatusCode::CONFLICT if is_write(request.method) => Some(Entry {
            authenticated: request.caller.is_some(),
            event: AuditEvent {
                action: "request.rejected",
                target: request.path.to_string(),
                outcome: AuditOutcome::Failed,
                detail: with(json!({})),
            },
        }),
        _ => None,
    }
}

fn is_write(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished<'a>(method: &'a Method, status: u16) -> Finished<'a> {
        Finished {
            method,
            path: "/api/subsystems",
            host: Some("evil.example"),
            status: StatusCode::from_u16(status).unwrap(),
            source: "10.0.0.1".into(),
            request_id: "r",
            caller: None,
            failure: None,
        }
    }

    #[test]
    fn only_refusals_and_rejected_writes_get_entries() {
        for (method, status, action) in [
            (Method::GET, 401, Some("auth.failed")),
            (Method::GET, 403, Some("access.denied")),
            (Method::GET, 421, Some("host.refused")),
            (Method::POST, 400, Some("request.rejected")),
            (Method::PUT, 409, Some("request.rejected")),
            (Method::GET, 400, None),
            (Method::GET, 409, None),
            (Method::GET, 404, None),
            (Method::GET, 200, None),
            (Method::POST, 201, None),
            (Method::GET, 429, None),
            (Method::POST, 500, None),
        ] {
            let entry = entry_for(&finished(&method, status));
            assert_eq!(entry.map(|e| e.event.action), action, "{method} {status}");
        }
    }

    #[test]
    fn the_host_is_cut_and_the_ticket_query_never_appears() {
        let long = "h".repeat(500);
        let method = Method::GET;
        let mut request = finished(&method, 421);
        request.host = Some(&long);
        let entry = entry_for(&request).unwrap();
        let host = entry.event.detail["host"].as_str().unwrap();
        assert_eq!(host.len(), MAX_HOST_CHARS);
        assert!(!entry.event.target.contains('?'));
    }

    #[test]
    fn denied_and_failed_outcomes_are_told_apart() {
        let get = Method::GET;
        let post = Method::POST;
        assert_eq!(
            entry_for(&finished(&get, 401)).unwrap().event.outcome,
            AuditOutcome::Denied
        );
        assert_eq!(
            entry_for(&finished(&post, 400)).unwrap().event.outcome,
            AuditOutcome::Failed
        );
    }
}
