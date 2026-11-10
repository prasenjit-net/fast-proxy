use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::envelope::Envelope;
use super::upstream_health::{summarize, UpstreamHealth};
use crate::state::SharedState;

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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    listeners: Vec<ListenerDto>,
    database: String,
    subsystems: SubsystemCounts,
    /// Endpoints in rotation and the ones that are not, with reasons.
    upstreams: UpstreamHealth,
    started_at_ms: i64,
    uptime_secs: u64,
}

/// `GET /api/status`: what is running and which config version is live.
pub async fn status(State(state): State<SharedState>) -> Json<Envelope<Status>> {
    let snapshot = state.config.snapshot_holder().load();
    let enabled = snapshot.subsystems.values().filter(|s| s.enabled).count();
    Json(Envelope {
        data: Status {
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
        },
        version: snapshot.version,
    })
}
