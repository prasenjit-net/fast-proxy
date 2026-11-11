use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{SubsystemId, UpstreamGroupId};
use fast_proxy_store::repository::{subsystems, upstreams};
use fast_proxy_store::Mutation;

use super::change::Change;
use super::detail::subsystem_name;
use super::envelope::{Envelope, VersionOnly};
use super::json::ApiJson;
use super::upstream_dto::{read_group, upstream_dto, UpstreamDto, UpstreamRequest};
use crate::state::SharedState;
use crate::AppResult;
use fast_proxy_config_service::Audited;

/// `GET /api/subsystems/{id}/upstreams`
pub async fn list_upstreams(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<Vec<UpstreamDto>>>> {
    let id = SubsystemId(id);
    let read = state
        .config
        .read(move |conn| subsystems::get(conn, id))
        .await?;
    Ok(Json(Envelope {
        data: read
            .value
            .upstream_groups
            .iter()
            .map(upstream_dto)
            .collect(),
        version: read.version,
    }))
}

/// `POST /api/subsystems/{id}/upstreams`: a group with its endpoints.
pub async fn create_upstream(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<UpstreamRequest>,
) -> AppResult<(StatusCode, Json<Envelope<UpstreamDto>>)> {
    let id = SubsystemId(id);
    let parsed = body.parse(true)?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new("upstream.create", format!("subsystem:{}", id.0)),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                let endpoints = parsed.endpoints.unwrap_or_default();
                let group = upstreams::create(conn, id, &parsed.fields, &endpoints)?;
                Ok(Mutation {
                    value: upstream_dto(&read_group(conn, id, group)?),
                    summary: format!("upstream created: {} in {subsystem}", parsed.fields.name),
                })
            },
        )
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data: applied.value,
            version: applied.version,
        }),
    ))
}

/// `PUT /api/subsystems/{id}/upstreams/{upstream_id}`: the group's name and
/// policy. Endpoints have their own resources.
pub async fn update_upstream(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<UpstreamRequest>,
) -> AppResult<Json<Envelope<UpstreamDto>>> {
    let (id, upstream_id) = (SubsystemId(id), UpstreamGroupId(upstream_id));
    let parsed = body.parse(false)?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "upstream.update",
                format!("subsystem:{}/upstream:{}", id.0, upstream_id.0),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                upstreams::update(conn, id, upstream_id, &parsed.fields)?;
                Ok(Mutation {
                    value: upstream_dto(&read_group(conn, id, upstream_id)?),
                    summary: format!("upstream updated: {} in {subsystem}", parsed.fields.name),
                })
            },
        )
        .await?;
    Ok(Json(Envelope {
        data: applied.value,
        version: applied.version,
    }))
}

/// `DELETE /api/subsystems/{id}/upstreams/{upstream_id}`
pub async fn delete_upstream(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, upstream_id) = (SubsystemId(id), UpstreamGroupId(upstream_id));
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "upstream.delete",
                format!("subsystem:{}/upstream:{}", id.0, upstream_id.0),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                upstreams::delete(conn, id, upstream_id)?;
                Ok(Mutation {
                    value: (),
                    summary: format!("upstream deleted from {subsystem}"),
                })
            },
        )
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
