//! JSON form of a [`Config`] as stored in a revision. It uses plain strings
//! and integers (not the validated model types) so the model crate needs no
//! serde, and re-validates everything on the way back in.

use fast_proxy_model::{
    parse_timeout_ms, parse_weight, Config, EndpointAddr, EndpointConfig, EndpointId,
    EndpointState, FailurePolicy, FieldError, GroupPolicy, HashKey, HealthCheck, HealthKind,
    HostPattern, LbAlgorithm, LbPolicy, PassiveHealth, RetryBudget, RetryPolicy, RouteConfig,
    RouteId, RoutePrefix, SubsystemConfig, SubsystemId, SubsystemName, UpstreamGroupConfig,
    UpstreamGroupId, UpstreamPrefix,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
    policy: PolicyJson,
    endpoints: Vec<EndpointJson>,
}

#[derive(Serialize, Deserialize)]
struct EndpointJson {
    id: i64,
    addr: String,
    weight: u32,
    state: String,
}

#[derive(Serialize, Deserialize)]
struct PolicyJson {
    lb_algorithm: String,
    lb_hash_key: String,
    failure_policy: String,
    health_check: String,
    health_path: String,
    health_status_min: u16,
    health_status_max: u16,
    health_interval_ms: u64,
    health_timeout_ms: u64,
    health_rise: u32,
    health_fall: u32,
    passive_failures: u32,
    passive_base_ms: u64,
    passive_max_ms: u64,
    passive_max_percent: u32,
    retry_budget_percent: u32,
    retry_budget_min_per_sec: u32,
}

#[derive(Serialize, Deserialize)]
struct RetryJson {
    max_attempts: u32,
    idempotent_on_5xx: bool,
    on_timeout: bool,
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
    retry: RetryJson,
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
                policy: policy_json(&group.policy),
                endpoints: group
                    .endpoints
                    .iter()
                    .map(|endpoint| EndpointJson {
                        id: endpoint.id.0,
                        addr: endpoint.addr.to_string(),
                        weight: endpoint.weight,
                        state: endpoint.state.as_str().to_string(),
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
                retry: RetryJson {
                    max_attempts: route.retry.max_attempts,
                    idempotent_on_5xx: route.retry.idempotent_on_5xx,
                    on_timeout: route.retry.on_timeout,
                },
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
                    policy: policy_from_json(&group.policy)?,
                    endpoints: group
                        .endpoints
                        .into_iter()
                        .map(|endpoint| {
                            Ok(EndpointConfig {
                                id: EndpointId(endpoint.id),
                                addr: EndpointAddr::parse(&endpoint.addr)?,
                                weight: parse_weight("weight", endpoint.weight)?,
                                state: EndpointState::parse(&endpoint.state)?,
                            })
                        })
                        .collect::<Result<_, FieldError>>()?,
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
                    retry: retry_from_json(&route.retry)?,
                    enabled: route.enabled,
                })
            })
            .collect::<Result<_, fast_proxy_model::FieldError>>()?,
    })
}

fn policy_json(policy: &GroupPolicy) -> PolicyJson {
    PolicyJson {
        lb_algorithm: policy.lb.algorithm.as_str().to_string(),
        lb_hash_key: policy.lb.hash_key.to_spec(),
        failure_policy: policy.failure.as_str().to_string(),
        health_check: policy.health.kind.as_str().to_string(),
        health_path: policy.health.path.clone(),
        health_status_min: policy.health.status_min,
        health_status_max: policy.health.status_max,
        health_interval_ms: millis(policy.health.interval),
        health_timeout_ms: millis(policy.health.timeout),
        health_rise: policy.health.rise,
        health_fall: policy.health.fall,
        passive_failures: policy.passive.consecutive_failures,
        passive_base_ms: millis(policy.passive.base_ejection),
        passive_max_ms: millis(policy.passive.max_ejection),
        passive_max_percent: policy.passive.max_ejection_percent,
        retry_budget_percent: policy.retry_budget.percent,
        retry_budget_min_per_sec: policy.retry_budget.min_per_sec,
    }
}

fn policy_from_json(json: &PolicyJson) -> Result<GroupPolicy, FieldError> {
    let policy = GroupPolicy {
        lb: LbPolicy {
            algorithm: LbAlgorithm::parse(&json.lb_algorithm)?,
            hash_key: HashKey::parse(&json.lb_hash_key)?,
        },
        failure: FailurePolicy::parse(&json.failure_policy)?,
        health: HealthCheck {
            kind: HealthKind::parse(&json.health_check)?,
            path: json.health_path.clone(),
            status_min: json.health_status_min,
            status_max: json.health_status_max,
            interval: Duration::from_millis(json.health_interval_ms),
            timeout: Duration::from_millis(json.health_timeout_ms),
            rise: json.health_rise,
            fall: json.health_fall,
        },
        passive: PassiveHealth {
            consecutive_failures: json.passive_failures,
            base_ejection: Duration::from_millis(json.passive_base_ms),
            max_ejection: Duration::from_millis(json.passive_max_ms),
            max_ejection_percent: json.passive_max_percent,
        },
        retry_budget: RetryBudget {
            percent: json.retry_budget_percent,
            min_per_sec: json.retry_budget_min_per_sec,
        },
    };
    policy.validate()?;
    Ok(policy)
}

fn retry_from_json(json: &RetryJson) -> Result<RetryPolicy, FieldError> {
    let retry = RetryPolicy {
        max_attempts: json.max_attempts,
        idempotent_on_5xx: json.idempotent_on_5xx,
        on_timeout: json.on_timeout,
    };
    retry.validate()?;
    Ok(retry)
}
