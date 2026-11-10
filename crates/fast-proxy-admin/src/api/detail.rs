use fast_proxy_config_service::ConfigService;
use fast_proxy_model::{RouteConfig, SubsystemConfig, SubsystemId};
use fast_proxy_store::repository::{hosts, subsystems};
use fast_proxy_store::{Connection, StoreError};
use serde::Serialize;

use super::envelope::Envelope;
use super::upstream_dto::{upstream_dto, UpstreamDto};
use crate::AppResult;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDto {
    pub id: i64,
    pub pattern: String,
}

/// A route's retry behaviour as shown by the API.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryDto {
    pub max_attempts: u32,
    pub idempotent_on_5xx: bool,
    pub on_timeout: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDto {
    pub id: i64,
    pub prefix: String,
    pub upstream_group_id: i64,
    pub strip_prefix: bool,
    pub upstream_prefix: String,
    pub preserve_host: bool,
    pub connect_timeout_ms: u64,
    pub response_timeout_ms: u64,
    pub retry: RetryDto,
    pub enabled: bool,
}

/// A subsystem with everything under it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemDetail {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub description: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub hosts: Vec<HostDto>,
    pub upstreams: Vec<UpstreamDto>,
    pub routes: Vec<RouteDto>,
}

/// A subsystem as shown in lists.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemSummary {
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    pub description: String,
    pub hosts: Vec<String>,
    pub route_count: usize,
    pub updated_at_ms: i64,
}

/// A duration as whole milliseconds, saturating.
pub fn millis_of(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub fn route_dto(route: &RouteConfig) -> RouteDto {
    RouteDto {
        id: route.id.0,
        prefix: route.prefix.as_str().to_string(),
        upstream_group_id: route.upstream_group_id.0,
        strip_prefix: route.strip_prefix,
        upstream_prefix: route.upstream_prefix.as_str().to_string(),
        preserve_host: route.preserve_host,
        connect_timeout_ms: millis_of(route.connect_timeout),
        response_timeout_ms: millis_of(route.response_timeout),
        retry: RetryDto {
            max_attempts: route.retry.max_attempts,
            idempotent_on_5xx: route.retry.idempotent_on_5xx,
            on_timeout: route.retry.on_timeout,
        },
        enabled: route.enabled,
    }
}

/// Routes in the order they are matched: longest prefix first.
pub fn routes_in_match_order(subsystem: &SubsystemConfig) -> Vec<RouteDto> {
    let mut routes: Vec<RouteDto> = subsystem.routes.iter().map(route_dto).collect();
    routes.sort_by(|a, b| {
        b.prefix
            .len()
            .cmp(&a.prefix.len())
            .then_with(|| a.prefix.cmp(&b.prefix))
    });
    routes
}

/// The subsystem's name, for revision summaries.
pub fn subsystem_name(conn: &Connection, id: SubsystemId) -> Result<String, StoreError> {
    Ok(subsystems::get(conn, id)?.name.as_str().to_string())
}

/// Loads one subsystem with its hosts, upstreams, routes, and timestamps.
pub async fn load_detail(
    config: &ConfigService,
    id: SubsystemId,
) -> AppResult<Envelope<SubsystemDetail>> {
    let read = config
        .read(move |conn| {
            let subsystem = subsystems::get(conn, id)?;
            let host_rows = hosts::list(conn, id)?;
            let times = subsystems::times(conn)?.remove(&id);
            Ok((subsystem, host_rows, times))
        })
        .await?;
    let (subsystem, host_rows, times) = read.value;
    let detail = SubsystemDetail {
        id: subsystem.id.0,
        name: subsystem.name.as_str().to_string(),
        enabled: subsystem.enabled,
        description: subsystem.description.clone(),
        created_at_ms: times.map_or(0, |t| t.created_at_ms),
        updated_at_ms: times.map_or(0, |t| t.updated_at_ms),
        hosts: host_rows
            .into_iter()
            .map(|(id, pattern)| HostDto {
                id: id.0,
                pattern: pattern.to_pattern_string(),
            })
            .collect(),
        upstreams: subsystem.upstream_groups.iter().map(upstream_dto).collect(),
        routes: routes_in_match_order(&subsystem),
    };
    Ok(Envelope {
        data: detail,
        version: read.version,
    })
}
