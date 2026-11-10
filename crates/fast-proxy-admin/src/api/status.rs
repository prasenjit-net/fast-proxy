use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::envelope::Envelope;
use super::upstream_health::{summarize, UpstreamHealth};
use crate::state::SharedState;
use crate::AppResult;

#[derive(Debug, Serialize)]
pub struct ListenerDto {
    name: String,
    addr: String,
}

#[derive(Debug, Serialize)]
pub struct SubsystemCounts {
    total: usize,
    enabled: usize,
}

/// The certificate that expires first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoonestExpiry {
    name: String,
    not_after_ms: i64,
    seconds_left: i64,
}

/// The process TLS state and the health of the certificates.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsSummary {
    enabled: bool,
    listen: Option<String>,
    /// `ok`, `key mismatch` or `not created`.
    vault: &'static str,
    certificates: usize,
    unreadable: usize,
    /// The soonest expiry among readable certificates, if there are any.
    soonest_expiry: Option<SoonestExpiry>,
}

/// How the admin plane is protected, and what operators have switched on.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdminSummary {
    /// `required` or `off`.
    auth: &'static str,
    /// Whether private keys may be sent over the admin channel.
    protected_channel: bool,
    /// Tokens that are neither revoked nor expired.
    active_tokens: usize,
    /// Maintenance scopes that are on right now.
    active_maintenance: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    tls: TlsSummary,
    listeners: Vec<ListenerDto>,
    database: String,
    subsystems: SubsystemCounts,
    /// Endpoints in rotation and the ones that are not, with reasons.
    upstreams: UpstreamHealth,
    started_at_ms: i64,
    uptime_secs: u64,
    admin: AdminSummary,
}

/// `GET /api/status`: what is running and which config version is live.
pub async fn status(State(state): State<SharedState>) -> AppResult<Json<Envelope<Status>>> {
    let snapshot = state.config.snapshot_holder().load();
    let enabled = snapshot.subsystems.values().filter(|s| s.enabled).count();
    let certificates = state.config.list_certificates().await?;
    let now = chrono::Utc::now().timestamp_millis();
    let active_tokens = state
        .config
        .list_tokens()
        .await?
        .iter()
        .filter(|token| {
            token.revoked_at_ms.is_none() && token.expires_at_ms.is_none_or(|at| at > now)
        })
        .count();
    let active_maintenance = state
        .config
        .list_maintenance()
        .await?
        .iter()
        .filter(|row| row.settings.is_active(now))
        .count();
    let soonest_expiry = certificates
        .iter()
        .filter(|certificate| certificate.state.as_str() != "unreadable")
        .min_by_key(|certificate| certificate.metadata.not_after_ms)
        .map(|certificate| SoonestExpiry {
            name: certificate.name.as_str().to_string(),
            not_after_ms: certificate.metadata.not_after_ms,
            seconds_left: (certificate.metadata.not_after_ms - now).div_euclid(1000),
        });
    Ok(Json(Envelope {
        data: Status {
            tls: TlsSummary {
                enabled: state.runtime.tls.enabled,
                listen: state.runtime.tls.listen.map(|addr| addr.to_string()),
                vault: state.config.vault_status().as_str(),
                certificates: certificates.len(),
                unreadable: certificates
                    .iter()
                    .filter(|certificate| certificate.state.as_str() == "unreadable")
                    .count(),
                soonest_expiry,
            },
            listeners: state
                .runtime
                .listeners
                .iter()
                .map(|listener| ListenerDto {
                    name: listener.name.clone(),
                    addr: listener.addr.to_string(),
                })
                .collect(),
            database: state.runtime.database_path.clone(),
            subsystems: SubsystemCounts {
                total: snapshot.subsystems.len(),
                enabled,
            },
            upstreams: summarize(&snapshot),
            started_at_ms: state.started_at_ms,
            uptime_secs: state.started.elapsed().as_secs(),
            admin: AdminSummary {
                auth: state.security.auth_mode(),
                protected_channel: state.security.channel_protected,
                active_tokens,
                active_maintenance,
            },
        },
        version: snapshot.version,
    }))
}
