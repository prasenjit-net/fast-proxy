use axum::extract::{Path, State};
use axum::Json;
use fast_proxy_balancing::monotonic_ms;
use fast_proxy_model::{SubsystemId, UpstreamGroupId};
use serde::Serialize;

use super::effective_state::Effective;
use super::envelope::Envelope;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// The latest active probe of an endpoint.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastProbe {
    ok: bool,
    ago_ms: u64,
}

/// An endpoint's live state: what the proxy is doing with it right now.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointStatus {
    id: i64,
    addr: String,
    weight: u32,
    /// The operator's decision: `enabled`, `draining` or `disabled`.
    state: &'static str,
    /// What the proxy does with it: `healthy`, `unhealthy`, `ejected`,
    /// `draining` or `disabled`.
    effective: &'static str,
    reason: String,
    in_flight: u32,
    /// Smoothed time to first byte; `null` before the first response.
    ewma_ms: Option<f64>,
    last_probe: Option<LastProbe>,
}

/// The runtime status of an upstream group.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamStatus {
    name: String,
    algorithm: &'static str,
    failure_policy: &'static str,
    /// How many endpoints are taking requests right now.
    healthy: usize,
    endpoints: Vec<EndpointStatus>,
}

/// `GET /api/subsystems/{id}/upstreams/{upstream_id}/status`: runtime state,
/// which is not configuration, so it comes from the live snapshot.
pub async fn upstream_status(
    State(state): State<SharedState>,
    Path((id, upstream_id)): Path<(i64, i64)>,
) -> AppResult<Json<Envelope<UpstreamStatus>>> {
    let snapshot = state.config.snapshot_holder().load();
    let upstream = snapshot
        .subsystems
        .get(&SubsystemId(id))
        .and_then(|subsystem| subsystem.upstreams.get(&UpstreamGroupId(upstream_id)))
        .ok_or_else(|| AppError::NotFound(format!("upstream {upstream_id} not found")))?;
    let now = monotonic_ms();
    let endpoints: Vec<EndpointStatus> = upstream
        .endpoints
        .iter()
        .zip(&upstream.candidates)
        .map(|(endpoint, candidate)| {
            let runtime = &candidate.runtime;
            let effective = Effective::classify(endpoint.state, runtime, now);
            EndpointStatus {
                id: endpoint.id.0,
                addr: endpoint.addr.to_string(),
                weight: endpoint.weight,
                state: endpoint.state.as_str(),
                effective: effective.name(),
                reason: effective.reason(),
                in_flight: runtime.in_flight(),
                ewma_ms: Some(runtime.ewma_us())
                    .filter(|micros| *micros > 0)
                    .map(|micros| micros as f64 / 1000.0),
                last_probe: runtime.last_probe().map(|(ok, at)| LastProbe {
                    ok,
                    ago_ms: now.saturating_sub(at),
                }),
            }
        })
        .collect();
    Ok(Json(Envelope {
        data: UpstreamStatus {
            name: upstream.name.clone(),
            algorithm: upstream.policy.lb.algorithm.as_str(),
            failure_policy: upstream.policy.failure.as_str(),
            healthy: endpoints
                .iter()
                .filter(|e| e.effective == "healthy")
                .count(),
            endpoints,
        },
        version: snapshot.version,
    }))
}
