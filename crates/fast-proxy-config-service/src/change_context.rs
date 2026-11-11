//! Who made a change, from where, and what it was: the audit side of
//! [`ConfigService::apply`](crate::ConfigService::apply). A caller cannot apply
//! a change without both, so no write goes unattributed.

use fast_proxy_store::repository::audit::{AuditOutcome, NewAuditEntry};

/// Longest summary kept in an audit entry's detail, in characters.
const MAX_SUMMARY_CHARS: usize = 500;

/// Who is making a change and where the request came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeContext {
    /// A token name, or `system` / `cli` / `anonymous`.
    pub actor: String,
    /// The actor's role (`admin`, `viewer`) or `system`.
    pub role: String,
    /// The client address, or `local` for the process itself.
    pub source: String,
    pub request_id: String,
}

impl ChangeContext {
    /// A change nobody asked for by name: startup, the sweeper, a reload.
    pub fn system() -> Self {
        Self::local("system")
    }

    /// A change made by the offline command line.
    pub fn cli() -> Self {
        Self::local("cli")
    }

    /// A request made while admin authentication is off.
    pub fn unauthenticated(source: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            actor: "anonymous".into(),
            role: "admin".into(),
            source: source.into(),
            request_id: request_id.into(),
        }
    }

    /// A request by an authenticated token.
    pub fn token(
        name: impl Into<String>,
        role: impl Into<String>,
        source: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            actor: name.into(),
            role: role.into(),
            source: source.into(),
            request_id: request_id.into(),
        }
    }

    fn local(actor: &str) -> Self {
        Self {
            actor: actor.into(),
            role: "system".into(),
            source: "local".into(),
            request_id: String::new(),
        }
    }
}

/// What a change was, as the audit log names it: a stable action such as
/// `subsystem.update` and the names it touched (never a body, secret or key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Audited {
    pub action: &'static str,
    pub target: String,
}

impl Audited {
    pub fn new(action: &'static str, target: impl Into<String>) -> Self {
        Self {
            action,
            target: target.into(),
        }
    }
}

/// The audit entry of a change that committed as revision `version`.
pub(crate) fn committed_entry(
    context: &ChangeContext,
    audited: &Audited,
    instance: &str,
    summary: &str,
    version: u64,
    at_ms: i64,
) -> NewAuditEntry {
    let summary: String = summary.chars().take(MAX_SUMMARY_CHARS).collect();
    NewAuditEntry {
        at_ms,
        instance: instance.to_string(),
        actor: context.actor.clone(),
        role: context.role.clone(),
        source: context.source.clone(),
        request_id: context.request_id.clone(),
        action: audited.action.to_string(),
        target: audited.target.clone(),
        outcome: AuditOutcome::Ok,
        detail: serde_json::json!({ "summary": summary }).to_string(),
        revision: Some(version),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_names_actor_source_action_target_and_revision() {
        let context = ChangeContext::token("ci", "admin", "10.0.0.5", "req-1");
        let entry = committed_entry(
            &context,
            &Audited::new("route.create", "shop/api"),
            "i-1",
            "route created: /api in shop",
            7,
            99,
        );
        assert_eq!(entry.actor, "ci");
        assert_eq!(entry.role, "admin");
        assert_eq!(entry.source, "10.0.0.5");
        assert_eq!(entry.request_id, "req-1");
        assert_eq!(entry.action, "route.create");
        assert_eq!(entry.target, "shop/api");
        assert_eq!(entry.revision, Some(7));
        assert_eq!(entry.instance, "i-1");
        assert_eq!(entry.at_ms, 99);
        assert_eq!(entry.outcome, AuditOutcome::Ok);
        assert_eq!(entry.detail, r#"{"summary":"route created: /api in shop"}"#);
    }

    #[test]
    fn a_long_summary_is_cut_so_the_detail_always_fits() {
        let entry = committed_entry(
            &ChangeContext::system(),
            &Audited::new("x.y", "t"),
            "i",
            &"é".repeat(5_000),
            1,
            0,
        );
        assert!(entry.detail.len() <= fast_proxy_store::repository::audit::MAX_DETAIL_BYTES);
    }

    #[test]
    fn the_local_contexts_are_distinct_actors() {
        assert_eq!(ChangeContext::system().actor, "system");
        assert_eq!(ChangeContext::cli().actor, "cli");
        assert_eq!(
            ChangeContext::unauthenticated("1.2.3.4", "r").actor,
            "anonymous"
        );
    }
}
