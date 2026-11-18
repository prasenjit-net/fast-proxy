//! Rate-limit policies of subsystems and routes (ADR 0030). A policy is
//! configuration: written through `ConfigService::apply`, audited, held by every
//! revision and restored by a rollback. A route's policy replaces its
//! subsystem's for that route; the routes without one share the subsystem's.
//! Limits are per instance: N instances admit up to N times the rate.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::Json;
use fast_proxy_model::{
    RateLimitHeader, RateLimitKey, RateLimitPolicy, RouteId, SubsystemId, DEFAULT_IPV6_PREFIX,
};
use fast_proxy_store::repository::{load, rate_limits};
use fast_proxy_store::{Mutation, StoreError};
use serde::{Deserialize, Serialize};

use super::change::Change;
use super::detail::subsystem_name;
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::{AppError, AppResult};
use fast_proxy_config_service::Audited;

const CLIENT: &str = "client";
const HEADER: &str = "header";

/// A policy as the API shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitDto {
    /// Requests allowed per `periodSecs`, per key, per instance.
    pub rate: u32,
    pub period_secs: u64,
    pub burst: u32,
    /// `client` (the effective client address) or `header`.
    pub key: &'static str,
    /// The header keyed on, for a `header` policy.
    pub header: Option<String>,
    pub ipv6_prefix: u8,
}

impl From<&RateLimitPolicy> for RateLimitDto {
    fn from(policy: &RateLimitPolicy) -> Self {
        let (key, header) = match &policy.key {
            RateLimitKey::Client => (CLIENT, None),
            RateLimitKey::Header(name) => (HEADER, Some(name.as_str().to_string())),
        };
        Self {
            rate: policy.rate,
            period_secs: policy.period.as_secs(),
            burst: policy.burst,
            key,
            header,
            ipv6_prefix: policy.ipv6_prefix,
        }
    }
}

/// Body of the `PUT .../rate-limit` routes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RateLimitRequest {
    rate: u32,
    period_secs: u32,
    /// Defaults to `rate`: a full period's worth at once.
    burst: Option<u32>,
    /// `client` (default) or `header`.
    key: Option<String>,
    /// Required for a `header` key.
    header: Option<String>,
    ipv6_prefix: Option<u8>,
}

impl RateLimitRequest {
    pub(super) fn into_policy(self) -> AppResult<RateLimitPolicy> {
        let mut errors = FieldErrors::default();
        let key = match (
            self.key.as_deref().unwrap_or(CLIENT),
            self.header.as_deref(),
        ) {
            (CLIENT, None) => Some(RateLimitKey::Client),
            (CLIENT, Some(_)) => {
                errors.add("rateLimit.header", "only a header key names a header");
                None
            }
            (HEADER, Some(name)) => errors
                .check(RateLimitHeader::parse("rateLimit.header", name))
                .map(RateLimitKey::Header),
            (HEADER, None) => {
                errors.add("rateLimit.header", "a header key needs the header's name");
                None
            }
            _ => {
                errors.add("rateLimit.key", "must be client or header");
                None
            }
        };
        let policy = key.map(|key| RateLimitPolicy {
            rate: self.rate,
            period: Duration::from_secs(u64::from(self.period_secs)),
            burst: self.burst.unwrap_or(self.rate),
            key,
            ipv6_prefix: self.ipv6_prefix.unwrap_or(DEFAULT_IPV6_PREFIX),
        });
        if let Some(policy) = &policy {
            errors.check(policy.validate());
        }
        errors.finish()?;
        policy.ok_or_else(|| AppError::Internal("a valid request yielded no policy".into()))
    }
}

/// `GET /api/subsystems/{id}/rate-limit`
pub async fn get_subsystem_rate_limit(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    read(&state, SubsystemId(id), None).await
}

/// `PUT /api/subsystems/{id}/rate-limit`
pub async fn set_subsystem_rate_limit(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<RateLimitRequest>,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    set(&state, &context, (SubsystemId(id), None), body).await
}

/// `DELETE /api/subsystems/{id}/rate-limit`
pub async fn clear_subsystem_rate_limit(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    clear(&state, &context, (SubsystemId(id), None)).await
}

/// `GET /api/subsystems/{id}/routes/{route_id}/rate-limit`
pub async fn get_route_rate_limit(
    State(state): State<SharedState>,
    Path((id, route_id)): Path<(i64, i64)>,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    read(&state, SubsystemId(id), Some(RouteId(route_id))).await
}

/// `PUT /api/subsystems/{id}/routes/{route_id}/rate-limit`
pub async fn set_route_rate_limit(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, route_id)): Path<(i64, i64)>,
    ApiJson(body): ApiJson<RateLimitRequest>,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    set(
        &state,
        &context,
        (SubsystemId(id), Some(RouteId(route_id))),
        body,
    )
    .await
}

/// `DELETE /api/subsystems/{id}/routes/{route_id}/rate-limit`
pub async fn clear_route_rate_limit(
    State(state): State<SharedState>,
    Change(context): Change,
    Path((id, route_id)): Path<(i64, i64)>,
) -> AppResult<Json<VersionOnly>> {
    clear(&state, &context, (SubsystemId(id), Some(RouteId(route_id)))).await
}

fn target(subsystem: SubsystemId, route: Option<RouteId>) -> String {
    match route {
        None => format!("subsystem:{}", subsystem.0),
        Some(route) => format!("subsystem:{}/route:{}", subsystem.0, route.0),
    }
}

async fn read(
    state: &SharedState,
    subsystem: SubsystemId,
    route: Option<RouteId>,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    let read = state.config.read(load::load_all).await?;
    let config = &read.value;
    let found = config
        .subsystems
        .iter()
        .find(|s| s.id == subsystem)
        .ok_or_else(|| StoreError::NotFound(format!("subsystem {}", subsystem.0)))?;
    let policy = match route {
        None => found.rate_limit.as_ref(),
        Some(route_id) => found
            .routes
            .iter()
            .find(|r| r.id == route_id)
            .ok_or_else(|| StoreError::NotFound(format!("route {}", route_id.0)))?
            .rate_limit
            .as_ref(),
    };
    let policy = policy.ok_or_else(|| StoreError::NotFound("rate limit".into()))?;
    Ok(Json(Envelope {
        data: RateLimitDto::from(policy),
        version: read.version,
    }))
}

async fn set(
    state: &SharedState,
    context: &fast_proxy_config_service::ChangeContext,
    (subsystem, route): (SubsystemId, Option<RouteId>),
    body: RateLimitRequest,
) -> AppResult<Json<Envelope<RateLimitDto>>> {
    let policy = body.into_policy()?;
    let data = RateLimitDto::from(&policy);
    let applied = state
        .config
        .apply(
            context,
            Audited::new("rate_limit.set", target(subsystem, route)),
            move |conn| {
                let name = subsystem_name(conn, subsystem)?;
                rate_limits::set(conn, subsystem, route, &policy)?;
                Ok(Mutation {
                    value: (),
                    summary: format!(
                        "rate limit set: {} per {}s on {name}{}",
                        policy.rate,
                        policy.period.as_secs(),
                        route.map_or(String::new(), |r| format!(" route {}", r.0))
                    ),
                })
            },
        )
        .await?;
    Ok(Json(Envelope {
        data,
        version: applied.version,
    }))
}

async fn clear(
    state: &SharedState,
    context: &fast_proxy_config_service::ChangeContext,
    (subsystem, route): (SubsystemId, Option<RouteId>),
) -> AppResult<Json<VersionOnly>> {
    let applied = state
        .config
        .apply(
            context,
            Audited::new("rate_limit.clear", target(subsystem, route)),
            move |conn| {
                let name = subsystem_name(conn, subsystem)?;
                if !rate_limits::clear(conn, subsystem, route)? {
                    return Err(StoreError::NotFound("rate limit".into()));
                }
                Ok(Mutation {
                    value: (),
                    summary: format!(
                        "rate limit cleared on {name}{}",
                        route.map_or(String::new(), |r| format!(" route {}", r.0))
                    ),
                })
            },
        )
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
