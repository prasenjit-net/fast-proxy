use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{FieldError, HostId, HostPattern, SubsystemId};
use fast_proxy_store::repository::hosts;
use fast_proxy_store::Mutation;
use serde::Deserialize;

use super::change::Change;
use super::detail::{subsystem_name, HostDto};
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;
use fast_proxy_config_service::Audited;

/// Body of `POST /api/subsystems/{id}/hosts`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostRequest {
    pattern: String,
}

/// `POST /api/subsystems/{id}/hosts`
pub async fn add_host(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<HostRequest>,
) -> AppResult<(StatusCode, Json<Envelope<HostDto>>)> {
    let id = SubsystemId(id);
    let mut errors = FieldErrors::default();
    // The model names this field "host"; the API calls it "pattern".
    let parsed =
        HostPattern::parse(&body.pattern).map_err(|err| FieldError::new("pattern", err.message));
    let pattern = errors.check(parsed);
    errors.finish()?;
    let Some(pattern) = pattern else {
        unreachable!("finish() fails when the pattern is invalid");
    };

    let text = pattern.to_pattern_string();
    let applied = state
        .config
        .apply(
            &context,
            Audited::new("host.add", format!("subsystem:{}", id.0)),
            move |conn| {
                let name = subsystem_name(conn, id)?;
                Ok(Mutation {
                    value: hosts::add(conn, id, &pattern)?,
                    summary: format!("host added: {} in {name}", pattern.to_pattern_string()),
                })
            },
        )
        .await?;
    let dto = HostDto {
        id: applied.value.0,
        pattern: text,
    };
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data: dto,
            version: applied.version,
        }),
    ))
}

/// `DELETE /api/subsystems/{id}/hosts/{host_id}`
pub async fn remove_host(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, host_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, host_id) = (SubsystemId(id), HostId(host_id));
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "host.remove",
                format!("subsystem:{}/host:{}", id.0, host_id.0),
            ),
            move |conn| {
                let name = subsystem_name(conn, id)?;
                hosts::remove(conn, id, host_id)?;
                Ok(Mutation {
                    value: (),
                    summary: format!("host removed from {name}"),
                })
            },
        )
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
