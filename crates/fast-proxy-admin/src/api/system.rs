//! Health, UI bootstrap config, live traffic, and Prometheus metrics endpoints.

use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{AppError, AppResult};
use crate::state::SharedState;
use crate::traffic::TrafficSnapshot;

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

/// `GET /api/traffic`: the latest live traffic snapshot (rates over the
/// last sampling window).
pub async fn traffic(State(state): State<SharedState>) -> AppResult<Json<TrafficSnapshot>> {
    state
        .latest_traffic
        .read()
        .await
        .clone()
        .map(Json)
        .ok_or_else(|| AppError::Internal("traffic statistics are not available yet".into()))
}

/// `GET /metrics`: Prometheus text exposition of the proxy's metrics.
pub async fn prometheus(State(state): State<SharedState>) -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        state.metrics.render(),
    )
}
