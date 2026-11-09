//! Health, UI bootstrap config, and metrics endpoints.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::live_metrics::MetricsSnapshot;
use crate::state::SharedState;

/// Liveness probe with the running version.
pub async fn health(State(state): State<SharedState>) -> Json<Value> {
    Json(json!({ "status": "ok", "version": state.version }))
}

/// Bootstrap configuration for the SPA, sourced from config.toml.
pub async fn config(State(state): State<SharedState>) -> Json<Value> {
    Json(json!({
        "ui": state.ui,
        "version": state.version,
        "startedAtMs": state.started_at_ms,
    }))
}

/// Latest sampled metrics snapshot.
pub async fn metrics(State(state): State<SharedState>) -> AppResult<Json<MetricsSnapshot>> {
    state
        .latest_metrics
        .read()
        .await
        .clone()
        .map(Json)
        .ok_or_else(|| AppError::Internal("metrics are not available yet".into()))
}
