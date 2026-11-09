//! JSON form of a [`Config`] as stored in a revision. It uses plain strings
//! and integers (not the validated model types) so the model crate needs no
//! serde, and re-validates everything on the way back in.

use fast_proxy_model::{
    parse_timeout_ms, Config, EndpointAddr, EndpointConfig, HostPattern, RouteConfig, RouteId,
    RoutePrefix, SubsystemConfig, SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId,
    UpstreamPrefix,
};
use serde::{Deserialize, Serialize};

use crate::StoreError;

#[derive(Serialize, Deserialize)]
struct ConfigJson {
    subsystems: Vec<SubsystemJson>,
}

#[derive(Serialize, Deserialize)]
struct SubsystemJson {
    id: i64,
    name: String,
    enabled: bool,
    description: String,
    hosts: Vec<String>,
    upstream_groups: Vec<GroupJson>,
    routes: Vec<RouteJson>,
}

#[derive(Serialize, Deserialize)]
struct GroupJson {
    id: i64,
    name: String,
    endpoints: Vec<EndpointJson>,
}

#[derive(Serialize, Deserialize)]
struct EndpointJson {
    addr: String,
    weight: u32,
}

#[derive(Serialize, Deserialize)]
struct RouteJson {
    id: i64,
    prefix: String,
    upstream_group_id: i64,
    strip_prefix: bool,
    upstream_prefix: String,
    preserve_host: bool,
    connect_timeout_ms: u64,
    response_timeout_ms: u64,
    enabled: bool,
}

/// Serializes a configuration for storage in a revision.
pub fn to_json(config: &Config) -> Result<String, StoreError> {
    let dto = ConfigJson {
        subsystems: config.subsystems.iter().map(subsystem_json).collect(),
    };
    serde_json::to_string(&dto).map_err(|err| StoreError::Internal(err.to_string()))
}

/// Parses and re-validates a stored configuration.
pub fn from_json(json: &str) -> Result<Config, StoreError> {
    let dto: ConfigJson = serde_json::from_str(json)
        .map_err(|err| StoreError::Internal(format!("stored revision is unreadable: {err}")))?;
    let subsystems = dto
        .subsystems
        .into_iter()
        .map(subsystem_from_json)
        .collect::<Result<_, _>>()
        .map_err(|err: fast_proxy_model::FieldError| {
            StoreError::Internal(format!("stored revision is invalid: {err}"))
        })?;
    Ok(Config { subsystems })
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn subsystem_json(subsystem: &SubsystemConfig) -> SubsystemJson {
    SubsystemJson {
        id: subsystem.id.0,
        name: subsystem.name.as_str().to_string(),
        enabled: subsystem.enabled,
        description: subsystem.description.clone(),
        hosts: subsystem
            .hosts
            .iter()
            .map(HostPattern::to_pattern_string)
            .collect(),
        upstream_groups: subsystem
            .upstream_groups
            .iter()
            .map(|group| GroupJson {
                id: group.id.0,
                name: group.name.clone(),
                endpoints: group
                    .endpoints
                    .iter()
                    .map(|endpoint| EndpointJson {
                        addr: endpoint.addr.to_string(),
                        weight: endpoint.weight,
                    })
                    .collect(),
            })
            .collect(),
        routes: subsystem
            .routes
            .iter()
            .map(|route| RouteJson {
                id: route.id.0,
                prefix: route.prefix.as_str().to_string(),
                upstream_group_id: route.upstream_group_id.0,
                strip_prefix: route.strip_prefix,
                upstream_prefix: route.upstream_prefix.as_str().to_string(),
                preserve_host: route.preserve_host,
                connect_timeout_ms: millis(route.connect_timeout),
                response_timeout_ms: millis(route.response_timeout),
                enabled: route.enabled,
            })
            .collect(),
    }
}

fn subsystem_from_json(
    json: SubsystemJson,
) -> Result<SubsystemConfig, fast_proxy_model::FieldError> {
    Ok(SubsystemConfig {
        id: SubsystemId(json.id),
        name: SubsystemName::parse(&json.name)?,
        enabled: json.enabled,
        description: json.description,
        hosts: json
            .hosts
            .iter()
            .map(|host| HostPattern::parse(host))
            .collect::<Result<_, _>>()?,
        upstream_groups: json
            .upstream_groups
            .into_iter()
            .map(|group| {
                Ok(UpstreamGroupConfig {
                    id: UpstreamGroupId(group.id),
                    name: group.name,
                    endpoints: group
                        .endpoints
                        .into_iter()
                        .map(|endpoint| {
                            Ok(EndpointConfig {
                                addr: EndpointAddr::parse(&endpoint.addr)?,
                                weight: endpoint.weight,
                            })
                        })
                        .collect::<Result<_, fast_proxy_model::FieldError>>()?,
                })
            })
            .collect::<Result<_, fast_proxy_model::FieldError>>()?,
        routes: json
            .routes
            .into_iter()
            .map(|route| {
                Ok(RouteConfig {
                    id: RouteId(route.id),
                    prefix: RoutePrefix::parse(&route.prefix)?,
                    upstream_group_id: UpstreamGroupId(route.upstream_group_id),
                    strip_prefix: route.strip_prefix,
                    upstream_prefix: UpstreamPrefix::parse(&route.upstream_prefix)?,
                    preserve_host: route.preserve_host,
                    connect_timeout: parse_timeout_ms(
                        "connect_timeout_ms",
                        route.connect_timeout_ms,
                    )?,
                    response_timeout: parse_timeout_ms(
                        "response_timeout_ms",
                        route.response_timeout_ms,
                    )?,
                    enabled: route.enabled,
                })
            })
            .collect::<Result<_, fast_proxy_model::FieldError>>()?,
    })
}
