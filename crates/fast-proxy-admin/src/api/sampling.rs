//! Sampling settings over the API (ADRs 0020 to 0023): the global settings
//! (triggers, thresholds, rates, redaction additions, retention) and the
//! per-subsystem and per-route overrides, plus the status a viewer may see. The
//! samples themselves are in `samples.rs` and need a stronger role.
//!
//! Settings are not part of the configuration: they are revisions with an
//! unchanged configuration copy and are never touched by a rollback.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::Json;
use fast_proxy_model::{
    Config, RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId, Trigger,
    TriggerSet,
};
use fast_proxy_sampling::{BUILT_IN_BODY_FIELDS, BUILT_IN_HEADERS, BUILT_IN_QUERY_PARAMS};
use fast_proxy_store::repository::load;
use fast_proxy_store::repository::sampling::SamplingOverrideRow;
use serde::{Deserialize, Serialize};

use super::change::Change;
use super::envelope::{Envelope, VersionOnly};
use super::input::{query_number, FieldErrors};
use super::json::ApiJson;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// Body of `PUT /api/sampling`: every setting, replacing the stored ones (so a
/// field left out is an error, never a quiet reset of a redaction list).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsRequest {
    enabled: bool,
    /// The names of the triggers that are on.
    triggers: Vec<String>,
    slow_threshold_ms: u32,
    slow_p99_factor_x10: u32,
    slow_p99_floor_ms: u32,
    client_errors_per_min: u32,
    oversized_request_bytes: u64,
    oversized_response_bytes: u64,
    baseline_per_mille: u32,
    rate_per_sec: u32,
    rate_burst: u32,
    storm_burst: u32,
    storm_interval_secs: u32,
    retention_days: u32,
    max_rows: u64,
    max_bytes: u64,
    body_request_cap: u32,
    body_response_cap: u32,
    extra_headers: Vec<String>,
    redact_headers: Vec<String>,
    redact_query_params: Vec<String>,
    redact_body_fields: Vec<String>,
}

/// The names that are redacted whatever the settings say.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuiltInRedaction {
    headers: &'static [&'static str],
    query_params: &'static [&'static str],
    body_fields: &'static [&'static str],
}

/// The global settings as the API shows them.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    enabled: bool,
    triggers: Vec<&'static str>,
    slow_threshold_ms: u32,
    slow_p99_factor_x10: u32,
    slow_p99_floor_ms: u32,
    client_errors_per_min: u32,
    oversized_request_bytes: u64,
    oversized_response_bytes: u64,
    baseline_per_mille: u32,
    rate_per_sec: u32,
    rate_burst: u32,
    storm_burst: u32,
    storm_interval_secs: u32,
    retention_days: u32,
    max_rows: u64,
    max_bytes: u64,
    body_request_cap: u32,
    body_response_cap: u32,
    extra_headers: Vec<String>,
    redact_headers: Vec<String>,
    redact_query_params: Vec<String>,
    redact_body_fields: Vec<String>,
    built_in_redaction: BuiltInRedaction,
    updated_by: String,
    updated_at_ms: i64,
}

fn settings_dto(settings: SamplingSettings) -> SettingsDto {
    SettingsDto {
        enabled: settings.enabled,
        triggers: settings.triggers.iter().map(Trigger::as_str).collect(),
        slow_threshold_ms: settings.slow_threshold_ms,
        slow_p99_factor_x10: settings.slow_p99_factor_x10,
        slow_p99_floor_ms: settings.slow_p99_floor_ms,
        client_errors_per_min: settings.client_errors_per_min,
        oversized_request_bytes: settings.oversized_request_bytes,
        oversized_response_bytes: settings.oversized_response_bytes,
        baseline_per_mille: settings.baseline_per_mille,
        rate_per_sec: settings.rate_per_sec,
        rate_burst: settings.rate_burst,
        storm_burst: settings.storm_burst,
        storm_interval_secs: settings.storm_interval_secs,
        retention_days: settings.retention_days,
        max_rows: settings.max_rows,
        max_bytes: settings.max_bytes,
        body_request_cap: settings.body_request_cap,
        body_response_cap: settings.body_response_cap,
        extra_headers: settings.extra_headers,
        redact_headers: settings.redact_headers,
        redact_query_params: settings.redact_query_params,
        redact_body_fields: settings.redact_body_fields,
        built_in_redaction: BuiltInRedaction {
            headers: &BUILT_IN_HEADERS,
            query_params: &BUILT_IN_QUERY_PARAMS,
            body_fields: &BUILT_IN_BODY_FIELDS,
        },
        updated_by: settings.updated_by,
        updated_at_ms: settings.updated_at_ms,
    }
}

/// One override as the API shows it, with the names of what it covers.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideDto {
    /// `subsystem` or `route`.
    scope: &'static str,
    subsystem_id: Option<i64>,
    subsystem: Option<String>,
    route_id: Option<i64>,
    route: Option<String>,
    enabled: Option<bool>,
    capture_bodies: Option<bool>,
    baseline_per_mille: Option<u32>,
    slow_threshold_ms: Option<u32>,
    slow_p99_factor_x10: Option<u32>,
    set_by: String,
    set_at_ms: i64,
}

fn override_dto(config: &Config, row: &SamplingOverrideRow) -> OverrideDto {
    let (scope, subsystem, route) = match row.target {
        SamplingTarget::Subsystem(id) => (
            "subsystem",
            config.subsystems.iter().find(|s| s.id == id),
            None,
        ),
        SamplingTarget::Route(id) => {
            let owner = config
                .subsystems
                .iter()
                .find(|s| s.routes.iter().any(|r| r.id == id));
            let route = owner.and_then(|s| s.routes.iter().find(|r| r.id == id));
            ("route", owner, route)
        }
    };
    OverrideDto {
        scope,
        subsystem_id: subsystem.map(|s| s.id.0),
        subsystem: subsystem.map(|s| s.name.as_str().to_string()),
        route_id: route.map(|r| r.id.0),
        route: route.map(|r| r.prefix.as_str().to_string()),
        enabled: row.settings.enabled,
        capture_bodies: row.settings.capture_bodies,
        baseline_per_mille: row.settings.baseline_per_mille,
        slow_threshold_ms: row.settings.slow_threshold_ms,
        slow_p99_factor_x10: row.settings.slow_p99_factor_x10,
        set_by: row.settings.set_by.clone(),
        set_at_ms: row.settings.set_at_ms,
    }
}

/// How much the shared sample table holds.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableDto {
    rows: u64,
    bytes: u64,
    oldest_at_ms: Option<i64>,
}

/// What a viewer may know about sampling: counts, never a sample.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    /// Request sampling runs in this process (`[sampling].enabled`, a restart
    /// to change).
    process_enabled: bool,
    /// Requests this process has kept and samples it has lost since it started.
    captured: u64,
    dropped: u64,
    /// The shared table, `None` when it could not be read just now.
    table: Option<TableDto>,
}

/// `GET /api/sampling` in full.
#[derive(Debug, Serialize)]
pub struct SamplingView {
    settings: SettingsDto,
    overrides: Vec<OverrideDto>,
    status: StatusDto,
}

async fn load_config(state: &SharedState) -> AppResult<Config> {
    Ok(state.config.read(load::load_all).await?.value)
}

async fn status(state: &SharedState) -> StatusDto {
    let (captured, dropped) = state.metrics.sample_totals();
    let reader = state.samples.clone();
    let table = tokio::task::spawn_blocking(move || reader.usage())
        .await
        .ok()
        .and_then(Result::ok)
        .map(|usage| TableDto {
            rows: usage.rows,
            bytes: usage.bytes,
            oldest_at_ms: usage.oldest_at_ms,
        });
    StatusDto {
        process_enabled: state.runtime.sampling_enabled,
        captured,
        dropped,
        table,
    }
}

/// `GET /api/sampling`: the settings, the overrides and the status.
pub async fn get_sampling(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<SamplingView>>> {
    let (settings, overrides) = state.config.sampling().await?;
    let config = load_config(&state).await?;
    Ok(Json(Envelope {
        data: SamplingView {
            settings: settings_dto(settings),
            overrides: overrides
                .iter()
                .map(|row| override_dto(&config, row))
                .collect(),
            status: status(&state).await,
        },
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `PUT /api/sampling`: replaces the global settings.
pub async fn put_sampling(
    State(state): State<SharedState>,
    Change(context): Change,
    ApiJson(body): ApiJson<SettingsRequest>,
) -> AppResult<Json<Envelope<SettingsDto>>> {
    let mut errors = FieldErrors::default();
    let triggers: TriggerSet = body
        .triggers
        .iter()
        .filter_map(|name| errors.check(Trigger::parse("triggers", name)))
        .collect();
    errors.finish()?;
    let settings = SamplingSettings {
        enabled: body.enabled,
        triggers,
        slow_threshold_ms: body.slow_threshold_ms,
        slow_p99_factor_x10: body.slow_p99_factor_x10,
        slow_p99_floor_ms: body.slow_p99_floor_ms,
        client_errors_per_min: body.client_errors_per_min,
        oversized_request_bytes: body.oversized_request_bytes,
        oversized_response_bytes: body.oversized_response_bytes,
        baseline_per_mille: body.baseline_per_mille,
        rate_per_sec: body.rate_per_sec,
        rate_burst: body.rate_burst,
        storm_burst: body.storm_burst,
        storm_interval_secs: body.storm_interval_secs,
        retention_days: body.retention_days,
        max_rows: body.max_rows,
        max_bytes: body.max_bytes,
        body_request_cap: body.body_request_cap,
        body_response_cap: body.body_response_cap,
        extra_headers: body.extra_headers,
        redact_headers: body.redact_headers,
        redact_query_params: body.redact_query_params,
        redact_body_fields: body.redact_body_fields,
        updated_by: String::new(),
        updated_at_ms: 0,
    };
    let applied = state.config.set_sampling(&context, settings).await?;
    let (stored, _) = state.config.sampling().await?;
    state.sampling_changed("settings");
    Ok(Json(Envelope {
        data: settings_dto(stored),
        version: applied.version,
    }))
}

/// Body of `PUT /api/sampling/overrides`: the target (one of `subsystemId` or
/// `routeId`) and the fields to override; an absent field inherits.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverrideRequest {
    subsystem_id: Option<i64>,
    route_id: Option<i64>,
    enabled: Option<bool>,
    capture_bodies: Option<bool>,
    baseline_per_mille: Option<u32>,
    slow_threshold_ms: Option<u32>,
    slow_p99_factor_x10: Option<u32>,
}

fn target_of(subsystem: Option<i64>, route: Option<i64>) -> AppResult<SamplingTarget> {
    match (subsystem, route) {
        (Some(id), None) => Ok(SamplingTarget::Subsystem(SubsystemId(id))),
        (None, Some(id)) => Ok(SamplingTarget::Route(RouteId(id))),
        _ => Err(AppError::Validation(vec![
            fast_proxy_model::FieldError::new(
                "subsystemId",
                "give exactly one of subsystemId and routeId",
            ),
        ])),
    }
}

/// `PUT /api/sampling/overrides`
pub async fn put_override(
    State(state): State<SharedState>,
    Change(context): Change,
    ApiJson(body): ApiJson<OverrideRequest>,
) -> AppResult<Json<Envelope<OverrideDto>>> {
    let target = target_of(body.subsystem_id, body.route_id)?;
    let settings = SamplingOverride {
        enabled: body.enabled,
        capture_bodies: body.capture_bodies,
        baseline_per_mille: body.baseline_per_mille,
        slow_threshold_ms: body.slow_threshold_ms,
        slow_p99_factor_x10: body.slow_p99_factor_x10,
        set_by: String::new(),
        set_at_ms: 0,
    };
    let applied = state
        .config
        .set_sampling_override(&context, target, settings)
        .await?;
    let (_, overrides) = state.config.sampling().await?;
    let row = overrides
        .iter()
        .find(|row| row.target == target)
        .ok_or_else(|| AppError::Internal("the override cannot be read back".into()))?;
    let data = override_dto(&load_config(&state).await?, row);
    state.sampling_changed("override");
    Ok(Json(Envelope {
        data,
        version: applied.version,
    }))
}

/// `DELETE /api/sampling/overrides?subsystemId=` or `?routeId=`
pub async fn delete_override(
    State(state): State<SharedState>,
    Change(context): Change,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<VersionOnly>> {
    let mut errors = FieldErrors::default();
    let subsystem = query_number(&params, "subsystemId", &mut errors).map(|id| id as i64);
    let route = query_number(&params, "routeId", &mut errors).map(|id| id as i64);
    errors.finish()?;
    let target = target_of(subsystem, route)?;
    let applied = state
        .config
        .clear_sampling_override(&context, target)
        .await?;
    if applied.value {
        state.sampling_changed("override");
    }
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
