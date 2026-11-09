use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{EndpointAddr, EndpointState, GroupPolicy, SubsystemId, UpstreamGroupId};
use fast_proxy_store::repository::{endpoints, subsystems, upstreams};
use fast_proxy_store::{EndpointFields, Mutation, StoreError, UpstreamFields};
use serde::Deserialize;

use super::detail::{subsystem_name, upstream_dto, UpstreamDto};
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;

const MAX_NAME_LEN: usize = 63;

/// Body of `POST` and `PUT` on a subsystem's upstreams.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamRequest {
    name: String,
    addr: String,
    #[serde(default = "default_weight")]
    weight: u32,
}

fn default_weight() -> u32 {
    1
}

/// A validated request: the group's name and its single endpoint (until the
/// admin API exposes multi-endpoint groups).
#[derive(Debug, Clone)]
struct ParsedUpstream {
    name: String,
    addr: EndpointAddr,
    weight: u32,
}

impl UpstreamRequest {
    fn into_parsed(self) -> AppResult<ParsedUpstream> {
        let mut errors = FieldErrors::default();
        let name = self.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
            errors.add("name", "must be 1 to 63 characters");
        }
        let addr = errors.check(EndpointAddr::parse(&self.addr));
        if self.weight == 0 {
            errors.add("weight", "must be at least 1");
        }
        errors.finish()?;
        let Some(addr) = addr else {
            unreachable!("finish() fails when the address is invalid");
        };
        Ok(ParsedUpstream {
            name,
            addr,
            weight: self.weight,
        })
    }
}

fn dto_from(id: UpstreamGroupId, parsed: &ParsedUpstream) -> UpstreamDto {
    UpstreamDto {
        id: id.0,
        name: parsed.name.clone(),
        addr: parsed.addr.to_string(),
        weight: parsed.weight,
    }
}

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

/// `POST /api/subsystems/{id}/upstreams`
pub async fn create_upstream(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<UpstreamRequest>,
) -> AppResult<(StatusCode, Json<Envelope<UpstreamDto>>)> {
    let id = SubsystemId(id);
    let parsed = body.into_parsed()?;
    let for_response = parsed.clone();
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            let fields = UpstreamFields {
                name: parsed.name.clone(),
                policy: GroupPolicy::default(),
            };
            let endpoint = EndpointFields {
                addr: parsed.addr,
                weight: parsed.weight,
                state: EndpointState::Enabled,
            };
            Ok(Mutation {
                value: upstreams::create(conn, id, &fields, &[endpoint])?,
                summary: format!("upstream created: {} in {subsystem}", fields.name),
            })
        })
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data: dto_from(applied.value, &for_response),
            version: applied.version,
        }),
    ))
}

/// `PUT /api/subsystems/{id}/upstreams/{upstream_id}`
pub async fn update_upstream(
    State(state): State<SharedState>,
    Path((id, upstream_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<UpstreamRequest>,
) -> AppResult<Json<Envelope<UpstreamDto>>> {
    let (id, upstream_id) = (SubsystemId(id), UpstreamGroupId(upstream_id));
    let parsed = body.into_parsed()?;
    let for_response = parsed.clone();
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            let current = subsystems::get(conn, id)?
                .upstream_groups
                .into_iter()
                .find(|group| group.id == upstream_id)
                .ok_or_else(|| StoreError::NotFound(format!("upstream {}", upstream_id.0)))?;
            let fields = UpstreamFields {
                name: parsed.name.clone(),
                policy: current.policy,
            };
            upstreams::update(conn, id, upstream_id, &fields)?;
            if let Some(endpoint) = current.endpoints.first() {
                let replacement = EndpointFields {
                    addr: parsed.addr.clone(),
                    weight: parsed.weight,
                    state: endpoint.state,
                };
                endpoints::update(conn, id, upstream_id, endpoint.id, &replacement)?;
            }
            Ok(Mutation {
                value: (),
                summary: format!("upstream updated: {} in {subsystem}", fields.name),
            })
        })
        .await?;
    Ok(Json(Envelope {
        data: dto_from(upstream_id, &for_response),
        version: applied.version,
    }))
}

/// `DELETE /api/subsystems/{id}/upstreams/{upstream_id}`
pub async fn delete_upstream(
    State(state): State<SharedState>,
    Path((id, upstream_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, upstream_id) = (SubsystemId(id), UpstreamGroupId(upstream_id));
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            upstreams::delete(conn, id, upstream_id)?;
            Ok(Mutation {
                value: (),
                summary: format!("upstream deleted from {subsystem}"),
            })
        })
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
