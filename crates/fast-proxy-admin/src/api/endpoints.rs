use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{EndpointId, EndpointState, SubsystemId, UpstreamGroupId};
use fast_proxy_store::repository::endpoints;
use fast_proxy_store::Mutation;
use serde::Deserialize;

use super::change::Change;
use super::detail::subsystem_name;
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use super::upstream_dto::{endpoint_dto, parse_endpoint, read_group, EndpointDto, EndpointRequest};
use crate::state::SharedState;
use crate::AppResult;
use fast_proxy_config_service::Audited;

/// Body of `PUT .../endpoints/{id}/state`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateRequest {
    state: String,
}

/// Reads one endpoint back after a change.
fn read_endpoint(
    conn: &fast_proxy_store::Connection,
    subsystem: SubsystemId,
    group: UpstreamGroupId,
    endpoint: EndpointId,
) -> Result<EndpointDto, fast_proxy_store::StoreError> {
    read_group(conn, subsystem, group)?
        .endpoints
        .iter()
        .find(|candidate| candidate.id == endpoint)
        .map(endpoint_dto)
        .ok_or_else(|| fast_proxy_store::StoreError::NotFound(format!("endpoint {}", endpoint.0)))
}

/// `POST /api/subsystems/{id}/upstreams/{upstream_id}/endpoints`
pub async fn add_endpoint(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<EndpointRequest>,
) -> AppResult<(StatusCode, Json<Envelope<EndpointDto>>)> {
    let (id, upstream_id) = (SubsystemId(id), UpstreamGroupId(upstream_id));
    let fields = single_endpoint(&body)?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "endpoint.add",
                format!("subsystem:{}/upstream:{}", id.0, upstream_id.0),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                let group = read_group(conn, id, upstream_id)?.name;
                let endpoint = endpoints::add(conn, id, upstream_id, &fields)?;
                Ok(Mutation {
                    value: read_endpoint(conn, id, upstream_id, endpoint)?,
                    summary: format!("endpoint added: {} to {group} in {subsystem}", fields.addr),
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

/// `PUT /api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}`
pub async fn update_endpoint(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id, endpoint_id)): Path<(i64, i64, i64)>,
    ApiJson(body): ApiJson<EndpointRequest>,
) -> AppResult<Json<Envelope<EndpointDto>>> {
    let (id, upstream_id, endpoint_id) = (
        SubsystemId(id),
        UpstreamGroupId(upstream_id),
        EndpointId(endpoint_id),
    );
    let fields = single_endpoint(&body)?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "endpoint.update",
                format!(
                    "subsystem:{}/upstream:{}/endpoint:{}",
                    id.0, upstream_id.0, endpoint_id.0
                ),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                endpoints::update(conn, id, upstream_id, endpoint_id, &fields)?;
                Ok(Mutation {
                    value: read_endpoint(conn, id, upstream_id, endpoint_id)?,
                    summary: format!("endpoint updated: {} in {subsystem}", fields.addr),
                })
            },
        )
        .await?;
    Ok(Json(Envelope {
        data: applied.value,
        version: applied.version,
    }))
}

/// `PUT /api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}/state`:
/// enable, drain or disable an endpoint.
pub async fn set_endpoint_state(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id, endpoint_id)): Path<(i64, i64, i64)>,
    ApiJson(body): ApiJson<StateRequest>,
) -> AppResult<Json<Envelope<EndpointDto>>> {
    let (id, upstream_id, endpoint_id) = (
        SubsystemId(id),
        UpstreamGroupId(upstream_id),
        EndpointId(endpoint_id),
    );
    let mut errors = FieldErrors::default();
    let new_state = errors.check(EndpointState::parse(&body.state));
    errors.finish()?;
    let Some(new_state) = new_state else {
        unreachable!("finish() fails when the state is invalid");
    };
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "endpoint.state",
                format!(
                    "subsystem:{}/upstream:{}/endpoint:{}",
                    id.0, upstream_id.0, endpoint_id.0
                ),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                endpoints::set_state(conn, id, upstream_id, endpoint_id, new_state)?;
                let endpoint = read_endpoint(conn, id, upstream_id, endpoint_id)?;
                Ok(Mutation {
                    summary: format!(
                        "endpoint {}: {} in {subsystem}",
                        new_state.as_str(),
                        endpoint.addr
                    ),
                    value: endpoint,
                })
            },
        )
        .await?;
    Ok(Json(Envelope {
        data: applied.value,
        version: applied.version,
    }))
}

/// `DELETE /api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}`;
/// the last endpoint of a group is a `409`.
pub async fn delete_endpoint(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, upstream_id, endpoint_id)): Path<(i64, i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, upstream_id, endpoint_id) = (
        SubsystemId(id),
        UpstreamGroupId(upstream_id),
        EndpointId(endpoint_id),
    );
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "endpoint.delete",
                format!(
                    "subsystem:{}/upstream:{}/endpoint:{}",
                    id.0, upstream_id.0, endpoint_id.0
                ),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                let addr = read_endpoint(conn, id, upstream_id, endpoint_id)?.addr;
                endpoints::delete(conn, id, upstream_id, endpoint_id)?;
                Ok(Mutation {
                    value: (),
                    summary: format!("endpoint removed: {addr} from {subsystem}"),
                })
            },
        )
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}

fn single_endpoint(body: &EndpointRequest) -> AppResult<fast_proxy_store::EndpointFields> {
    let mut errors = FieldErrors::default();
    let fields = parse_endpoint(body, "", &mut errors);
    errors.finish()?;
    let Some(fields) = fields else {
        unreachable!("finish() fails when the endpoint is invalid");
    };
    Ok(fields)
}
