//! Sampling settings: the global row and the per-subsystem and per-route
//! overrides. Like maintenance and tokens they are not part of `Config`: they
//! go through [`ConfigService::apply`] as a revision with an unchanged
//! configuration copy (which bumps the cluster generation, so other instances
//! follow within the sync interval), they are audited in the same transaction,
//! and a rollback never changes them.
//!
//! The samples themselves are not written here: they are telemetry, written by
//! the sample service without a revision (ADR 0022).

use fast_proxy_model::{SamplingOverride, SamplingSettings, SamplingTarget};
use fast_proxy_routing::{SamplingInputs, SamplingRuntime};
use fast_proxy_store::repository::sampling::{self, SamplingOverrideRow};
use fast_proxy_store::{Connection, Mutation, StoreError};

use crate::service::{now_ms, NOTHING_TO_DO};
use crate::{Applied, Audited, ChangeContext, ConfigService};

impl ConfigService {
    /// The stored global settings (the defaults before the first write) and
    /// every override.
    pub async fn sampling(
        &self,
    ) -> Result<(SamplingSettings, Vec<SamplingOverrideRow>), StoreError> {
        Ok(self
            .read(|conn| {
                Ok((
                    sampling::get_settings(conn)?,
                    sampling::list_overrides(conn)?,
                ))
            })
            .await?
            .value)
    }

    /// Replaces the global sampling settings. Who changed them and when come
    /// from `context` and the clock, whatever `settings` says.
    pub async fn set_sampling(
        &self,
        context: &ChangeContext,
        mut settings: SamplingSettings,
    ) -> Result<Applied<()>, StoreError> {
        settings.updated_by = context.actor.clone();
        settings.updated_at_ms = now_ms();
        self.apply(
            context,
            Audited::new("sampling.update", "global"),
            move |conn| {
                sampling::set_settings(conn, &settings)?;
                Ok(Mutation {
                    value: (),
                    summary: "sampling settings updated".into(),
                })
            },
        )
        .await
    }

    /// Sets the override of a subsystem or route, replacing an earlier one.
    pub async fn set_sampling_override(
        &self,
        context: &ChangeContext,
        target: SamplingTarget,
        mut settings: SamplingOverride,
    ) -> Result<Applied<()>, StoreError> {
        settings.set_by = context.actor.clone();
        settings.set_at_ms = now_ms();
        let described = describe(target);
        let summary = format!("sampling override set: {described}");
        self.apply(
            context,
            Audited::new("sampling.override.set", described),
            move |conn| {
                sampling::set_override(conn, target, &settings)?;
                Ok(Mutation { value: (), summary })
            },
        )
        .await
    }

    /// Removes the override of `target`. Returns whether there was one: removing
    /// what is not there changes nothing and writes nothing.
    pub async fn clear_sampling_override(
        &self,
        context: &ChangeContext,
        target: SamplingTarget,
    ) -> Result<Applied<bool>, StoreError> {
        let described = describe(target);
        let summary = format!("sampling override cleared: {described}");
        let result = self
            .apply(
                context,
                Audited::new("sampling.override.clear", described),
                move |conn| {
                    if !sampling::clear_override(conn, target)? {
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
}

/// The stored sampling settings as the compile step takes them, carrying
/// `runtime` (the guards and each route's rolling latency) over.
pub(crate) fn stored_inputs(
    conn: &Connection,
    runtime: SamplingRuntime,
) -> Result<SamplingInputs, StoreError> {
    let overrides = sampling::list_overrides(conn)?
        .into_iter()
        .map(|row| (row.target, row.settings))
        .collect();
    Ok(SamplingInputs {
        settings: sampling::get_settings(conn)?,
        overrides,
        runtime,
    })
}

/// The audit and summary spelling of a target: ids, which stay valid after the
/// subsystem or route is renamed.
fn describe(target: SamplingTarget) -> String {
    match target {
        SamplingTarget::Subsystem(id) => format!("subsystem:{}", id.0),
        SamplingTarget::Route(id) => format!("route:{}", id.0),
    }
}
