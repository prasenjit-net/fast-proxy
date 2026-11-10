use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{SubsystemId, SubsystemName};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Mutation, SubsystemFields};
use serde::Deserialize;

use super::detail::{load_detail, subsystem_name, SubsystemDetail, SubsystemSummary};
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;

const MAX_DESCRIPTION_LEN: usize = 500;

/// Body of `POST /api/subsystems` and `PUT /api/subsystems/{id}`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubsystemRequest {
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    description: String,
}

fn enabled_by_default() -> bool {
    true
}

impl SubsystemRequest {
    fn into_fields(self) -> AppResult<SubsystemFields> {
        let mut errors = FieldErrors::default();
        let name = errors.check(SubsystemName::parse(&self.name));
        if self.description.chars().count() > MAX_DESCRIPTION_LEN {
            errors.add("description", "must be at most 500 characters");
        }
        errors.finish()?;
        let Some(name) = name else {
            unreachable!("finish() fails when the name is invalid");
        };
        Ok(SubsystemFields {
            name,
            enabled: self.enabled,
            description: self.description,
            https: fast_proxy_model::HttpsPolicy::default(),
        })
    }
}

/// `GET /api/subsystems`
pub async fn list_subsystems(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<Vec<SubsystemSummary>>>> {
    let read = state
        .config
        .read(|conn| Ok((subsystems::list(conn)?, subsystems::times(conn)?)))
        .await?;
    let (all, times) = read.value;
    let summaries = all
        .iter()
        .map(|subsystem| SubsystemSummary {
            id: subsystem.id.0,
            name: subsystem.name.as_str().to_string(),
            enabled: subsystem.enabled,
            description: subsystem.description.clone(),
            hosts: subsystem
                .hosts
                .iter()
                .map(|h| h.to_pattern_string())
                .collect(),
            route_count: subsystem.routes.len(),
            updated_at_ms: times.get(&subsystem.id).map_or(0, |t| t.updated_at_ms),
        })
        .collect();
    Ok(Json(Envelope {
        data: summaries,
        version: read.version,
    }))
}

/// `POST /api/subsystems`
pub async fn create_subsystem(
    State(state): State<SharedState>,
    ApiJson(body): ApiJson<SubsystemRequest>,
) -> AppResult<(StatusCode, Json<Envelope<SubsystemDetail>>)> {
    let fields = body.into_fields()?;
    let name = fields.name.as_str().to_string();
    let applied = state
        .config
        .apply(move |conn| {
            Ok(Mutation {
                value: subsystems::create(conn, &fields)?,
                summary: format!("subsystem created: {name}"),
            })
        })
        .await?;
    let mut detail = load_detail(&state.config, applied.value).await?;
    detail.version = applied.version;
    Ok((StatusCode::CREATED, Json(detail)))
}

/// `GET /api/subsystems/{id}`
pub async fn get_subsystem(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<SubsystemDetail>>> {
    Ok(Json(load_detail(&state.config, SubsystemId(id)).await?))
}

/// `PUT /api/subsystems/{id}`
pub async fn update_subsystem(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<SubsystemRequest>,
) -> AppResult<Json<Envelope<SubsystemDetail>>> {
    let id = SubsystemId(id);
    let fields = body.into_fields()?;
    let applied = state
        .config
        .apply(move |conn| {
            subsystems::update(conn, id, &fields)?;
            Ok(Mutation {
                value: (),
                summary: format!("subsystem updated: {}", fields.name.as_str()),
            })
        })
        .await?;
    let mut detail = load_detail(&state.config, id).await?;
    detail.version = applied.version;
    Ok(Json(detail))
}

/// `DELETE /api/subsystems/{id}`
pub async fn delete_subsystem(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    let id = SubsystemId(id);
    let applied = state
        .config
        .apply(move |conn| {
            let name = subsystem_name(conn, id)?;
            subsystems::delete(conn, id)?;
            Ok(Mutation {
                value: (),
                summary: format!("subsystem deleted: {name}"),
            })
        })
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
