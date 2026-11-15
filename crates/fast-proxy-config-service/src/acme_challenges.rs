//! Publishing and clearing ACME HTTP-01 challenges (ADR 0025). Challenges are
//! not `Config`, but every instance must answer them, so they go through
//! [`ConfigService::apply`] as a summary-only revision: the writing instance
//! answers at once, the others after their next sync poll.

use fast_proxy_model::{AcmeChallenge, SubsystemId};
use fast_proxy_routing::AcmeInputs;
use fast_proxy_store::repository::acme::challenges;
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Connection, Mutation, StoreError};

use crate::service::{now_ms, NOTHING_TO_DO};
use crate::{Applied, Audited, ChangeContext, ConfigService};

/// The stored challenges, for compile, as of `now_ms`.
pub(crate) fn stored_inputs(conn: &Connection, now_ms: i64) -> Result<AcmeInputs, StoreError> {
    Ok(AcmeInputs {
        challenges: challenges::list(conn)?,
        now_ms,
    })
}

impl ConfigService {
    /// Publishes the challenges of one order for `subsystem`, replacing any it
    /// had, and drops expired ones of any subsystem. Written by `system`
    /// (`acme.challenge.publish`).
    pub async fn publish_acme_challenges(
        &self,
        subsystem: SubsystemId,
        published: Vec<AcmeChallenge>,
    ) -> Result<Applied<()>, StoreError> {
        if published.iter().any(|c| c.subsystem_id != subsystem) {
            return Err(StoreError::invalid_field(
                "challenges",
                "every challenge must belong to the subsystem",
            ));
        }
        self.apply(
            &ChangeContext::system(),
            Audited::new(
                "acme.challenge.publish",
                format!("subsystem:{}", subsystem.0),
            ),
            move |conn| {
                let name = subsystems::get(conn, subsystem)?.name;
                challenges::clear_expired(conn, now_ms())?;
                challenges::clear_subsystem(conn, subsystem)?;
                challenges::put(conn, &published)?;
                Ok(Mutation {
                    value: (),
                    summary: format!(
                        "acme challenges published: {} ({})",
                        name.as_str(),
                        published.len()
                    ),
                })
            },
        )
        .await
    }

    /// Removes `subsystem`'s challenges after a failed order. Returns whether
    /// there were any: with none, nothing is written.
    pub async fn clear_acme_challenges(
        &self,
        subsystem: SubsystemId,
    ) -> Result<Applied<bool>, StoreError> {
        let result = self
            .apply(
                &ChangeContext::system(),
                Audited::new("acme.challenge.clear", format!("subsystem:{}", subsystem.0)),
                move |conn| {
                    if challenges::clear_subsystem(conn, subsystem)? == 0 {
                        return Err(StoreError::Conflict(NOTHING_TO_DO.into()));
                    }
                    let name = subsystems::get(conn, subsystem)?.name;
                    Ok(Mutation {
                        value: true,
                        summary: format!("acme challenges cleared: {}", name.as_str()),
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
