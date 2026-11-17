use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{
    check_max_body_bytes, parse_timeout_ms, RetryPolicy, RouteId, RoutePrefix, SubsystemId,
    UpstreamGroupId, UpstreamPrefix,
};
use fast_proxy_store::repository::{routes, subsystems};
use fast_proxy_store::{Mutation, RouteFields};
use serde::Deserialize;

use super::change::Change;
use super::detail::{route_dto, routes_in_match_order, subsystem_name, RouteDto};
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;
use fast_proxy_config_service::Audited;

const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_RESPONSE_TIMEOUT_MS: u64 = 30_000;

/// Body of `POST` and `PUT` on a subsystem's routes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteRequest {
    prefix: String,
    upstream_group_id: i64,
    #[serde(default)]
    strip_prefix: bool,
    #[serde(default)]
    upstream_prefix: String,
    #[serde(default = "yes")]
    preserve_host: bool,
    #[serde(default = "default_connect_timeout")]
    connect_timeout_ms: u64,
    #[serde(default = "default_response_timeout")]
    response_timeout_ms: u64,
    #[serde(default)]
    retry: RetryRequest,
    #[serde(default = "yes")]
    enabled: bool,
    /// The largest request body; absent or `null` uses the process default.
    max_body_bytes: Option<u64>,
}

/// A route's retry settings; absent fields take their defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryRequest {
    max_attempts: Option<u32>,
    idempotent_on_5xx: Option<bool>,
    on_timeout: Option<bool>,
}

fn yes() -> bool {
    true
}

fn default_connect_timeout() -> u64 {
    DEFAULT_CONNECT_TIMEOUT_MS
}

fn default_response_timeout() -> u64 {
    DEFAULT_RESPONSE_TIMEOUT_MS
}

impl RouteRequest {
    /// The route's fields and its body cap, validated together.
    fn into_fields(self) -> AppResult<(RouteFields, Option<u64>)> {
        let mut errors = FieldErrors::default();
        if let Some(max) = self.max_body_bytes {
            errors.check(check_max_body_bytes("maxBodyBytes", max));
        }
        let prefix = errors.check(RoutePrefix::parse(&self.prefix));
        let upstream_prefix = errors.check(UpstreamPrefix::parse(&self.upstream_prefix));
        let connect_timeout = errors.check(parse_timeout_ms(
            "connect_timeout_ms",
            self.connect_timeout_ms,
        ));
        let response_timeout = errors.check(parse_timeout_ms(
            "response_timeout_ms",
            self.response_timeout_ms,
        ));
        let defaults = RetryPolicy::default();
        let retry = RetryPolicy {
            max_attempts: self.retry.max_attempts.unwrap_or(defaults.max_attempts),
            idempotent_on_5xx: self
                .retry
                .idempotent_on_5xx
                .unwrap_or(defaults.idempotent_on_5xx),
            on_timeout: self.retry.on_timeout.unwrap_or(defaults.on_timeout),
        };
        errors.check(retry.validate());
        errors.finish()?;
        let (Some(prefix), Some(upstream_prefix), Some(connect_timeout), Some(response_timeout)) =
            (prefix, upstream_prefix, connect_timeout, response_timeout)
        else {
            unreachable!("finish() fails when any field is invalid");
        };
        let fields = RouteFields {
            prefix,
            upstream_group_id: UpstreamGroupId(self.upstream_group_id),
            strip_prefix: self.strip_prefix,
            upstream_prefix,
            preserve_host: self.preserve_host,
            connect_timeout,
            response_timeout,
            retry,
            enabled: self.enabled,
        };
        Ok((fields, self.max_body_bytes))
    }
}

/// The route as stored, for the response to a write: the same as a `GET`.
fn stored_route(
    conn: &fast_proxy_store::Connection,
    id: SubsystemId,
    route_id: RouteId,
) -> Result<RouteDto, fast_proxy_store::StoreError> {
    subsystems::get(conn, id)?
        .routes
        .iter()
        .find(|route| route.id == route_id)
        .map(route_dto)
        .ok_or_else(|| fast_proxy_store::StoreError::NotFound(format!("route {}", route_id.0)))
}

/// `GET /api/subsystems/{id}/routes`, in match order (longest prefix first).
pub async fn list_routes(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<Vec<RouteDto>>>> {
    let id = SubsystemId(id);
    let read = state
        .config
        .read(move |conn| subsystems::get(conn, id))
        .await?;
    Ok(Json(Envelope {
        data: routes_in_match_order(&read.value),
        version: read.version,
    }))
}

/// `POST /api/subsystems/{id}/routes`
pub async fn create_route(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RouteRequest>,
) -> AppResult<(StatusCode, Json<Envelope<RouteDto>>)> {
    let id = SubsystemId(id);
    let (fields, max_body_bytes) = body.into_fields()?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new("route.create", format!("subsystem:{}", id.0)),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                let route_id = routes::create(conn, id, &fields)?;
                if max_body_bytes.is_some() {
                    routes::set_max_body_bytes(conn, id, route_id, max_body_bytes)?;
                }
                Ok(Mutation {
                    value: stored_route(conn, id, route_id)?,
                    summary: format!("route created: {} in {subsystem}", fields.prefix.as_str()),
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

/// `PUT /api/subsystems/{id}/routes/{route_id}`
pub async fn update_route(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, route_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<RouteRequest>,
) -> AppResult<Json<Envelope<RouteDto>>> {
    let (id, route_id) = (SubsystemId(id), RouteId(route_id));
    let (fields, max_body_bytes) = body.into_fields()?;
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "route.update",
                format!("subsystem:{}/route:{}", id.0, route_id.0),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                routes::update(conn, id, route_id, &fields)?;
                routes::set_max_body_bytes(conn, id, route_id, max_body_bytes)?;
                Ok(Mutation {
                    value: stored_route(conn, id, route_id)?,
                    summary: format!("route updated: {} in {subsystem}", fields.prefix.as_str()),
                })
            },
        )
        .await?;
    Ok(Json(Envelope {
        data: applied.value,
        version: applied.version,
    }))
}

/// `DELETE /api/subsystems/{id}/routes/{route_id}`
pub async fn delete_route(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, route_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, route_id) = (SubsystemId(id), RouteId(route_id));
    let applied = state
        .config
        .apply(
            &context,
            Audited::new(
                "route.delete",
                format!("subsystem:{}/route:{}", id.0, route_id.0),
            ),
            move |conn| {
                let subsystem = subsystem_name(conn, id)?;
                routes::delete(conn, id, route_id)?;
                Ok(Mutation {
                    value: (),
                    summary: format!("route deleted from {subsystem}"),
                })
            },
        )
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
