//! Maintenance writes and the expiry sweeper. Maintenance is not part of
//! `Config`: it goes through [`ConfigService::apply`] as a revision with an
//! unchanged configuration copy (which bumps the cluster generation, so other
//! instances follow within the sync interval), and a rollback never changes it.
//!
//! A scope's expiry is exact because the data plane checks the clock at request
//! time; the sweeper below only makes the database, the UI and other instances
//! agree afterwards by clearing the expired rows with a normal write.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{MaintenanceSettings, MaintenanceTarget};
use fast_proxy_routing::MaintenanceInputs;
use fast_proxy_store::repository::maintenance::{self, MaintenanceRow};
use fast_proxy_store::{Mutation, StoreError};
use tokio_util::sync::CancellationToken;

use crate::service::{now_ms, NOTHING_TO_DO};
use crate::{Applied, Audited, ChangeContext, ConfigService};

impl ConfigService {
    /// Turns maintenance on for `target`, or replaces its settings. Who set it
    /// and when come from `context` and the clock, whatever `settings` says.
    pub async fn set_maintenance(
        &self,
        context: &ChangeContext,
        target: MaintenanceTarget,
        mut settings: MaintenanceSettings,
    ) -> Result<Applied<()>, StoreError> {
        settings.set_by = context.actor.clone();
        settings.set_at_ms = now_ms();
        let described = describe(target);
        let summary = format!("maintenance on: {described}");
        self.apply(
            context,
            Audited::new("maintenance.set", described),
            move |conn| {
                maintenance::set(conn, target, &settings)?;
                Ok(Mutation { value: (), summary })
            },
        )
        .await
    }

    /// Turns maintenance off for `target`. Returns whether it was on: turning
    /// off what is already off changes nothing and writes nothing.
    pub async fn clear_maintenance(
        &self,
        context: &ChangeContext,
        target: MaintenanceTarget,
    ) -> Result<Applied<bool>, StoreError> {
        let described = describe(target);
        let summary = format!("maintenance off: {described}");
        let result = self
            .apply(
                context,
                Audited::new("maintenance.clear", described),
                move |conn| {
                    if !maintenance::clear(conn, target)? {
                        return Err(StoreError::Conflict(NOTHING_TO_DO.into()));
                    }
                    Ok(Mutation {
                        value: true,
                        summary,
                    })
                },
            )
            .await;
        match result {
            Err(StoreError::Conflict(message)) if message == NOTHING_TO_DO => Ok(Applied {
                value: false,
                version: self.snapshot_holder().load().version,
            }),
            other => other,
        }
    }

    /// Every stored scope, expired ones included until the sweeper clears them.
    pub async fn list_maintenance(&self) -> Result<Vec<MaintenanceRow>, StoreError> {
        Ok(self.read(maintenance::list).await?.value)
    }

    /// Clears every scope whose expiry has passed, as one change by `system`
    /// with the audit action `maintenance.expired`, and returns how many.
    /// Writes nothing when none expired. Every instance runs this; the second
    /// finds nothing left, so the database sees each expiry once (two
    /// instances sweeping in the very same instant may both record it, which
    /// is harmless).
    pub async fn sweep_expired_maintenance(&self) -> Result<usize, StoreError> {
        let now = now_ms();
        let expired = expired_targets(&self.list_maintenance().await?, now);
        if expired.is_empty() {
            return Ok(0);
        }
        let described: Vec<String> = expired.iter().copied().map(describe).collect();
        let summary = format!("maintenance expired: {}", described.join(", "));
        let result = self
            .apply(
                &ChangeContext::system(),
                Audited::new("maintenance.expired", described.join(", ")),
                move |conn| {
                    let mut cleared = 0;
                    for target in expired {
                        if maintenance::clear(conn, target)? {
                            cleared += 1;
                        }
                    }
                    if cleared == 0 {
                        return Err(StoreError::Conflict(NOTHING_TO_DO.into()));
                    }
                    Ok(Mutation {
                        value: cleared,
                        summary,
                    })
                },
            )
            .await;
        match result {
            Ok(applied) => Ok(applied.value),
            Err(StoreError::Conflict(message)) if message == NOTHING_TO_DO => Ok(0),
            Err(err) => Err(err),
        }
    }

    /// Runs [`sweep_expired_maintenance`](Self::sweep_expired_maintenance) every
    /// `interval` until `shutdown` is cancelled. A zero interval turns it off.
    pub async fn run_maintenance_sweeper(
        self: Arc<Self>,
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
            if let Err(err) = self.sweep_expired_maintenance().await {
                tracing::warn!("could not clear expired maintenance: {err}");
            }
        }
    }
}

/// The stored scopes as the compile step takes them, with the time they are
/// compiled at.
pub(crate) fn stored_inputs(
    conn: &fast_proxy_store::Connection,
    now_ms: i64,
) -> Result<MaintenanceInputs, StoreError> {
    let scopes = maintenance::list(conn)?
        .into_iter()
        .map(|row| (row.target, row.settings))
        .collect();
    Ok(MaintenanceInputs { scopes, now_ms })
}

fn expired_targets(rows: &[MaintenanceRow], now_ms: i64) -> Vec<MaintenanceTarget> {
    rows.iter()
        .filter(|row| !row.settings.is_active(now_ms))
        .map(|row| row.target)
        .collect()
}

/// The audit and summary spelling of a target: ids, which stay valid after the
/// subsystem or route is renamed.
fn describe(target: MaintenanceTarget) -> String {
    match target {
        MaintenanceTarget::Global => "global".into(),
        MaintenanceTarget::Subsystem(id) => format!("subsystem:{}", id.0),
        MaintenanceTarget::Route(id) => format!("route:{}", id.0),
    }
}
