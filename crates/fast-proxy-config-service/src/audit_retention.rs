//! Keeping the audit log bounded: entries older than the retention age, and
//! entries beyond the row limit, are pruned in small batches so no single
//! statement holds the writer lock for long. The newest rows are never removed
//! by the count rule.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_store::repository::audit;
use fast_proxy_store::StoreError;
use tokio_util::sync::CancellationToken;

use crate::service::now_ms;
use crate::ConfigService;

/// Rows deleted per statement.
const PRUNE_BATCH: u32 = 1_000;

/// How much audit history to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditRetention {
    pub max_age: Duration,
    pub max_rows: u32,
}

impl ConfigService {
    /// Prunes the audit log to `retention` as of `now_ms`; returns how many
    /// entries were removed.
    pub async fn prune_audit_log(
        &self,
        retention: AuditRetention,
        now_ms: i64,
    ) -> Result<usize, StoreError> {
        let max_age_ms = i64::try_from(retention.max_age.as_millis()).unwrap_or(i64::MAX);
        let cutoff = now_ms.saturating_sub(max_age_ms);
        let mut removed = 0;
        loop {
            let batch = self
                .store
                .run(move |db| {
                    db.read(|conn| audit::prune(conn, cutoff, retention.max_rows, PRUNE_BATCH))
                })
                .await?;
            if batch == 0 {
                return Ok(removed);
            }
            removed += batch;
        }
    }

    /// Prunes every `interval` until `shutdown` is cancelled. A zero interval
    /// turns it off.
    pub async fn run_audit_pruner(
        self: Arc<Self>,
        retention: AuditRetention,
        interval: Duration,
        shutdown: CancellationToken,
    ) {
        if interval.is_zero() {
            return;
        }
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            if let Err(err) = self.prune_audit_log(retention, now_ms()).await {
                tracing::warn!("could not prune the audit log: {err}");
            }
        }
    }
}
