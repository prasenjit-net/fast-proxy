//! ACME over the API (M4, A1): the process view, one subsystem's status with
//! the names an order would cover now, and asking for a renewal. Statuses are
//! read from the database, so every instance shows the same thing. No key,
//! credential or CA URL beyond the directory is ever returned.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_acme::{eligible_names, RenewalPolicy};
use fast_proxy_config_service::{AuditEvent, CertificateSummary};
use fast_proxy_model::{AcmeState, AcmeStatus, SkippedName, SubsystemConfig, SubsystemId};
use fast_proxy_store::repository::audit::AuditOutcome;
use fast_proxy_store::repository::{load, subsystems};
use serde::Serialize;

use super::change::Change;
use super::envelope::Envelope;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// An order started this long ago and still `pending`/`renewing` is taken to
/// be running; older, it was left by an instance that stopped.
const RUNNING_FOR_MS: i64 = 10 * 60_000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedDto {
    name: String,
    reason: &'static str,
}

/// One subsystem's ACME status.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    subsystem_id: i64,
    subsystem: String,
    state: &'static str,
    eligible_names: Vec<String>,
    skipped_names: Vec<SkippedDto>,
    certificate_id: Option<i64>,
    certificate_name: Option<String>,
    not_after_ms: Option<i64>,
    next_attempt_at_ms: Option<i64>,
    failures: u32,
    last_error: String,
    last_attempt_at_ms: Option<i64>,
    last_success_at_ms: Option<i64>,
    instance: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseDto {
    holder: String,
    expires_at_ms: i64,
    /// This instance holds it.
    held_here: bool,
}

/// `GET /api/acme`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcmeOverview {
    enabled: bool,
    directory: Option<String>,
    staging: bool,
    allowed_domains: Vec<String>,
    account_registered: bool,
    lease: Option<LeaseDto>,
    subsystems: Vec<StatusDto>,
}

/// What an order for a subsystem would cover now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDto {
    eligible_names: Vec<String>,
    skipped_names: Vec<SkippedDto>,
}

/// `GET /api/subsystems/{id}/acme`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemAcme {
    /// ACME is on for the process.
    available: bool,
    /// The subsystem's switch.
    acme_enabled: bool,
    status: Option<StatusDto>,
    preview: Option<PreviewDto>,
}

/// `GET /api/acme`: the process settings, the account, the lease and every
/// subsystem's status.
pub async fn acme_overview(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<AcmeOverview>>> {
    let version = state.config.snapshot_holder().load().version;
    let Some(acme) = &state.acme else {
        return Ok(Json(Envelope {
            version,
            data: AcmeOverview {
                enabled: false,
                directory: None,
                staging: false,
                allowed_domains: Vec::new(),
                account_registered: false,
                lease: None,
                subsystems: Vec::new(),
            },
        }));
    };
    let config = state.config.read(load::load_all).await?.value;
    let certificates = state.config.list_certificates().await?;
    let statuses = state.config.acme_statuses().await?;
    let lease = state.config.acme_lease().await?.map(|lease| LeaseDto {
        held_here: lease.holder == state.config.instance_id(),
        holder: lease.holder,
        expires_at_ms: lease.expires_at_ms,
    });
    let names: HashMap<SubsystemId, &SubsystemConfig> =
        config.subsystems.iter().map(|s| (s.id, s)).collect();
    let subsystems = statuses
        .iter()
        .filter_map(|status| {
            let subsystem = names.get(&status.subsystem_id)?;
            Some(status_dto(status, subsystem, &certificates))
        })
        .collect();
    Ok(Json(Envelope {
        version,
        data: AcmeOverview {
            enabled: true,
            directory: Some(acme.directory.clone()),
            staging: acme.staging,
            allowed_domains: acme.allowlist.patterns(),
            account_registered: state
                .config
                .acme_account_registered(&acme.directory)
                .await?,
            lease,
            subsystems,
        },
    }))
}

/// `GET /api/subsystems/{id}/acme`: the status, and what an order would cover now.
pub async fn subsystem_acme(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<SubsystemAcme>>> {
    let id = SubsystemId(id);
    let subsystem = state
        .config
        .read(move |conn| subsystems::get(conn, id))
        .await?
        .value;
    let certificates = state.config.list_certificates().await?;
    let status = state
        .config
        .acme_status(id)
        .await?
        .map(|status| status_dto(&status, &subsystem, &certificates));
    let preview = state.acme.as_ref().map(|acme| {
        let eligibility = eligible_names(&subsystem.hosts, &acme.allowlist);
        PreviewDto {
            eligible_names: eligibility.eligible,
            skipped_names: skipped(&eligibility.skipped),
        }
    });
    Ok(Json(Envelope {
        version: state.config.snapshot_holder().load().version,
        data: SubsystemAcme {
            available: state.acme.is_some(),
            acme_enabled: subsystem.https.enabled && subsystem.https.acme_enabled,
            status,
            preview,
        },
    }))
}

/// `POST /api/subsystems/{id}/acme/renew`: asks the runner to order now,
/// skipping the backoff. Refused while an order runs (409), when the hourly
/// failure cap is spent (429), and when ACME is not on for the subsystem (409).
/// Audited.
pub async fn renew_acme(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<StatusCode> {
    let acme = state
        .acme
        .as_ref()
        .ok_or_else(|| AppError::Conflict("ACME is not enabled for this proxy".into()))?;
    let id = SubsystemId(id);
    let subsystem = state
        .config
        .read(move |conn| subsystems::get(conn, id))
        .await?
        .value;
    if !(subsystem.https.enabled && subsystem.https.acme_enabled) {
        return Err(AppError::Conflict(
            "ACME is not on for this subsystem (https.acmeEnabled)".into(),
        ));
    }
    let now = chrono::Utc::now().timestamp_millis();
    if let Some(status) = state.config.acme_status(id).await? {
        refuse_while_busy(&status, now)?;
    }
    if !acme.control.request_renewal(id) {
        return Err(AppError::Conflict(
            "the ACME runner is busy; try again shortly".into(),
        ));
    }
    state
        .config
        .record_audit(
            &context,
            AuditEvent {
                action: "acme.renew.request",
                target: format!("subsystem:{}", id.0),
                outcome: AuditOutcome::Ok,
                detail: serde_json::json!({ "subsystem": subsystem.name.as_str() }),
            },
        )
        .await;
    Ok(StatusCode::ACCEPTED)
}

/// 409 while an order runs, 429 while the hourly cap is spent.
fn refuse_while_busy(status: &AcmeStatus, now: i64) -> AppResult<()> {
    let running = matches!(status.state, AcmeState::Pending | AcmeState::Renewing)
        && status
            .last_attempt_at_ms
            .is_some_and(|at| now - at < RUNNING_FOR_MS);
    if running {
        return Err(AppError::Conflict(
            "an order for this subsystem is running".into(),
        ));
    }
    if let Some(until) = RenewalPolicy::default().cap_ends_at(status, now) {
        return Err(AppError::RateLimited {
            message: "too many failed orders in the last hour; wait before forcing another".into(),
            retry_after_secs: u64::try_from((until - now) / 1000 + 1).unwrap_or(1),
        });
    }
    Ok(())
}

fn status_dto(
    status: &AcmeStatus,
    subsystem: &SubsystemConfig,
    certificates: &[CertificateSummary],
) -> StatusDto {
    let certificate = status
        .certificate_id
        .and_then(|id| certificates.iter().find(|c| c.id == id));
    StatusDto {
        subsystem_id: status.subsystem_id.0,
        subsystem: subsystem.name.as_str().to_string(),
        state: status.state.as_str(),
        eligible_names: status.eligible_names.clone(),
        skipped_names: skipped(&status.skipped_names),
        certificate_id: status.certificate_id.map(|id| id.0),
        certificate_name: certificate.map(|c| c.name.as_str().to_string()),
        not_after_ms: certificate.map(|c| c.metadata.not_after_ms),
        next_attempt_at_ms: status.next_attempt_at_ms,
        failures: status.failures,
        last_error: status.last_error.clone(),
        last_attempt_at_ms: status.last_attempt_at_ms,
        last_success_at_ms: status.last_success_at_ms,
        instance: status.instance.clone(),
    }
}

fn skipped(names: &[SkippedName]) -> Vec<SkippedDto> {
    names
        .iter()
        .map(|s| SkippedDto {
            name: s.name.clone(),
            reason: s.reason.as_str(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recent_order_in_progress_is_busy_and_an_old_one_is_not() {
        let mut status = AcmeStatus::new(SubsystemId(1));
        status.state = AcmeState::Renewing;
        status.last_attempt_at_ms = Some(1_000);
        assert!(matches!(
            refuse_while_busy(&status, 2_000),
            Err(AppError::Conflict(_))
        ));
        assert!(refuse_while_busy(&status, 1_000 + RUNNING_FOR_MS).is_ok());
    }

    #[test]
    fn a_spent_cap_is_429_with_the_seconds_left() {
        let mut status = AcmeStatus::new(SubsystemId(1));
        status.state = AcmeState::Failed;
        status.window_failures = 5;
        status.failure_window_start_ms = Some(0);
        match refuse_while_busy(&status, 3_000_000) {
            Err(AppError::RateLimited {
                retry_after_secs, ..
            }) => assert_eq!(retry_after_secs, 601),
            other => panic!("{other:?}"),
        }
    }
}
