//! Audit entries that are not committed changes: refusals, rejected writes and
//! token use. A committed change writes its own entry in its own transaction
//! ([`ConfigService::apply`](crate::ConfigService::apply)); these never do, so
//! nothing is recorded twice and nothing here creates a revision.

use fast_proxy_model::TokenId;
use fast_proxy_store::repository::audit::{self, AuditOutcome, NewAuditEntry};
use fast_proxy_store::repository::tokens;
use fast_proxy_store::StoreError;

use crate::service::now_ms;
use crate::{ChangeContext, ConfigService};

/// Longest `detail` kept; the store refuses more than 2 KiB.
const MAX_DETAIL_BYTES: usize = 1_900;

/// An event for the audit log that is not a committed change.
#[derive(Debug, Clone)]
pub struct AuditEvent {
    /// A stable name such as `auth.failed` or `access.denied`.
    pub action: &'static str,
    /// What it concerned: a path, a token, a name. Never a body, secret or key.
    pub target: String,
    pub outcome: AuditOutcome,
    /// Short JSON.
    pub detail: serde_json::Value,
}

impl ConfigService {
    /// Appends `event` to the audit log as `context`. Nothing but a log line
    /// reports a failure to write it: auditing must never break the request it
    /// describes.
    pub async fn record_audit(&self, context: &ChangeContext, event: AuditEvent) {
        let mut detail = event.detail.to_string();
        if detail.len() > MAX_DETAIL_BYTES {
            detail = serde_json::json!({ "truncated": true }).to_string();
        }
        let entry = NewAuditEntry {
            at_ms: now_ms(),
            instance: self.instance.clone(),
            actor: context.actor.clone(),
            role: context.role.clone(),
            source: context.source.clone(),
            request_id: context.request_id.clone(),
            action: event.action.to_string(),
            target: event.target,
            outcome: event.outcome,
            detail,
            revision: None,
        };
        if let Err(err) = self
            .store
            .run(move |db| db.read(|conn| audit::append(conn, &entry)))
            .await
        {
            tracing::warn!("could not write an audit entry: {err}");
        }
    }

    /// Notes that token `id` was used, at most once a minute (the store keeps
    /// `last_used_at` coarse), and records its first use ever in the audit log.
    pub async fn record_token_use(&self, context: &ChangeContext, id: TokenId) {
        let used = self
            .store
            .run(move |db| db.read(|conn| tokens::mark_used(conn, id, now_ms())))
            .await;
        match used {
            Ok(true) => {
                self.record_audit(
                    context,
                    AuditEvent {
                        action: "token.first_use",
                        target: format!("token:{}", id.0),
                        outcome: AuditOutcome::Ok,
                        detail: serde_json::json!({}),
                    },
                )
                .await;
            }
            Ok(false) => {}
            Err(StoreError::NotFound(_)) => {}
            Err(err) => tracing::warn!("could not note a token's use: {err}"),
        }
    }
}
