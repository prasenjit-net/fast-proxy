use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{
    parse_timeout_ms, RetryPolicy, RouteId, RoutePrefix, SubsystemId, UpstreamGroupId,
    UpstreamPrefix,
};
use fast_proxy_store::repository::{routes, subsystems};
use fast_proxy_store::{Mutation, RouteFields};
use serde::Deserialize;

use super::detail::{millis_of, routes_in_match_order, subsystem_name, RetryDto, RouteDto};
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;

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
    fn into_fields(self) -> AppResult<RouteFields> {
        let mut errors = FieldErrors::default();
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
        Ok(RouteFields {
            prefix,
            upstream_group_id: UpstreamGroupId(self.upstream_group_id),
            strip_prefix: self.strip_prefix,
            upstream_prefix,
            preserve_host: self.preserve_host,
            connect_timeout,
            response_timeout,
            retry,
            enabled: self.enabled,
        })
    }
}

fn dto_from(id: RouteId, fields: &RouteFields) -> RouteDto {
    RouteDto {
        id: id.0,
        prefix: fields.prefix.as_str().to_string(),
        upstream_group_id: fields.upstream_group_id.0,
        strip_prefix: fields.strip_prefix,
        upstream_prefix: fields.upstream_prefix.as_str().to_string(),
        preserve_host: fields.preserve_host,
        connect_timeout_ms: millis_of(fields.connect_timeout),
        response_timeout_ms: millis_of(fields.response_timeout),
        retry: RetryDto {
            max_attempts: fields.retry.max_attempts,
            idempotent_on_5xx: fields.retry.idempotent_on_5xx,
            on_timeout: fields.retry.on_timeout,
        },
        enabled: fields.enabled,
    }
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
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RouteRequest>,
) -> AppResult<(StatusCode, Json<Envelope<RouteDto>>)> {
    let id = SubsystemId(id);
    let fields = body.into_fields()?;
    let for_response = fields.clone();
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            Ok(Mutation {
                value: routes::create(conn, id, &fields)?,
                summary: format!("route created: {} in {subsystem}", fields.prefix.as_str()),
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

/// `PUT /api/subsystems/{id}/routes/{route_id}`
pub async fn update_route(
    State(state): State<SharedState>,
    Path((id, route_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<RouteRequest>,
) -> AppResult<Json<Envelope<RouteDto>>> {
    let (id, route_id) = (SubsystemId(id), RouteId(route_id));
    let fields = body.into_fields()?;
    let for_response = fields.clone();
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            routes::update(conn, id, route_id, &fields)?;
            Ok(Mutation {
                value: (),
                summary: format!("route updated: {} in {subsystem}", fields.prefix.as_str()),
            })
        })
        .await?;
    Ok(Json(Envelope {
        data: dto_from(route_id, &for_response),
        version: applied.version,
    }))
}

/// `DELETE /api/subsystems/{id}/routes/{route_id}`
pub async fn delete_route(
    State(state): State<SharedState>,
    Path((id, route_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    let (id, route_id) = (SubsystemId(id), RouteId(route_id));
    let applied = state
        .config
        .apply(move |conn| {
            let subsystem = subsystem_name(conn, id)?;
            routes::delete(conn, id, route_id)?;
            Ok(Mutation {
                value: (),
                summary: format!("route deleted from {subsystem}"),
            })
        })
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
