//! `GET /api/limits`: the process limits in force on this instance, how full the
//! rate-limit key table is, and what the limits refused so far (ADR 0029). A
//! viewer may read it; nothing here is secret. The numbers are this instance's
//! own: every instance has its own connections, keys and counters.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::envelope::Envelope;
use crate::state::SharedState;
use crate::AppResult;

/// The limits of this instance, with how they are doing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsStatus {
    pub settings: LimitSettings,
    pub rate_limit_keys: KeyTableFill,
    /// Connections refused when accepted, by cap.
    pub connections_refused: Vec<ReasonCount>,
    /// Requests a limit refused, by reason.
    pub requests_refused: Vec<ReasonCount>,
    /// Clients ended for making no progress, by kind.
    pub client_timeouts: Vec<ReasonCount>,
    /// New clients let through because the key table was full.
    pub rate_limit_table_full: u64,
}

/// `[limits]` as running. Times in milliseconds; `null` is off or unlimited.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitSettings {
    pub max_connections: usize,
    pub max_connections_per_client: Option<usize>,
    pub header_read_timeout_ms: u64,
    pub body_read_timeout_ms: u64,
    pub send_timeout_ms: u64,
    pub tunnel_idle_timeout_ms: Option<u64>,
    pub max_header_count: usize,
    pub max_header_bytes: usize,
    pub max_uri_bytes: usize,
    pub max_body_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyTableFill {
    pub used: usize,
    pub capacity: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasonCount {
    pub reason: String,
    pub count: u64,
}

fn counts(values: Vec<(String, u64)>) -> Vec<ReasonCount> {
    values
        .into_iter()
        .map(|(reason, count)| ReasonCount { reason, count })
        .collect()
}

/// `GET /api/limits`
pub async fn limits_status(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<LimitsStatus>>> {
    let limits = &state.runtime.limits;
    let snapshot = state.config.snapshot_holder().load();
    let budget = snapshot.rate_limit_runtime().budget().clone();
    let counted = state.metrics.limit_counts();
    let data = LimitsStatus {
        settings: LimitSettings {
            max_connections: limits.max_connections,
            max_connections_per_client: (limits.max_connections_per_client > 0)
                .then_some(limits.max_connections_per_client),
            header_read_timeout_ms: limits.header_read_timeout_ms,
            body_read_timeout_ms: limits.body_read_timeout_ms,
            send_timeout_ms: limits.send_timeout_ms,
            tunnel_idle_timeout_ms: limits.tunnel_idle_timeout_ms,
            max_header_count: limits.max_header_count,
            max_header_bytes: limits.max_header_bytes,
            max_uri_bytes: limits.max_uri_bytes,
            max_body_bytes: limits.max_body_bytes,
        },
        rate_limit_keys: KeyTableFill {
            used: budget.used(),
            capacity: budget.capacity(),
        },
        connections_refused: counts(counted.connections_refused),
        requests_refused: counts(counted.requests_refused),
        client_timeouts: counts(counted.client_timeouts),
        rate_limit_table_full: counted.rate_limit_table_full,
    };
    Ok(Json(Envelope {
        data,
        version: snapshot.version,
    }))
}
