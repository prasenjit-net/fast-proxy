//! The upstream group resource: its JSON shape, and parsing a request body
//! into validated store inputs. Nested field errors use dotted camelCase
//! paths (`health.intervalMs`, `endpoints.0.addr`).

use fast_proxy_model::{
    parse_weight, EndpointAddr, EndpointConfig, EndpointState, FailurePolicy, FieldError,
    GroupPolicy, HashKey, HealthCheck, HealthKind, LbAlgorithm, LbPolicy, PassiveHealth,
    RetryBudget, SubsystemId, UpstreamGroupConfig, UpstreamGroupId, MAX_ENDPOINTS_PER_GROUP,
};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Connection, EndpointFields, StoreError, UpstreamFields};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::detail::millis_of;
use super::input::FieldErrors;
use crate::AppResult;

const MAX_NAME_LEN: usize = 63;

// ---- responses ------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointDto {
    pub id: i64,
    pub addr: String,
    pub weight: u32,
    pub state: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LbDto {
    algorithm: &'static str,
    hash_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthDto {
    check: &'static str,
    path: String,
    expected_status: [u16; 2],
    interval_ms: u64,
    timeout_ms: u64,
    rise: u32,
    fall: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PassiveDto {
    consecutive_failures: u32,
    base_ejection_ms: u64,
    max_ejection_ms: u64,
    max_ejection_percent: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryBudgetDto {
    percent: u32,
    min_per_sec: u32,
}

/// An upstream group with its policy and endpoints.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamDto {
    pub id: i64,
    pub name: String,
    lb: LbDto,
    failure_policy: &'static str,
    health: HealthDto,
    passive: PassiveDto,
    retry_budget: RetryBudgetDto,
    pub endpoints: Vec<EndpointDto>,
}

pub fn endpoint_dto(endpoint: &EndpointConfig) -> EndpointDto {
    EndpointDto {
        id: endpoint.id.0,
        addr: endpoint.addr.to_string(),
        weight: endpoint.weight,
        state: endpoint.state.as_str(),
    }
}

pub fn upstream_dto(group: &UpstreamGroupConfig) -> UpstreamDto {
    let policy = &group.policy;
    UpstreamDto {
        id: group.id.0,
        name: group.name.clone(),
        lb: LbDto {
            algorithm: policy.lb.algorithm.as_str(),
            hash_key: policy.lb.hash_key.to_spec(),
        },
        failure_policy: policy.failure.as_str(),
        health: HealthDto {
            check: policy.health.kind.as_str(),
            path: policy.health.path.clone(),
            expected_status: [policy.health.status_min, policy.health.status_max],
            interval_ms: millis_of(policy.health.interval),
            timeout_ms: millis_of(policy.health.timeout),
            rise: policy.health.rise,
            fall: policy.health.fall,
        },
        passive: PassiveDto {
            consecutive_failures: policy.passive.consecutive_failures,
            base_ejection_ms: millis_of(policy.passive.base_ejection),
            max_ejection_ms: millis_of(policy.passive.max_ejection),
            max_ejection_percent: policy.passive.max_ejection_percent,
        },
        retry_budget: RetryBudgetDto {
            percent: policy.retry_budget.percent,
            min_per_sec: policy.retry_budget.min_per_sec,
        },
        endpoints: group.endpoints.iter().map(endpoint_dto).collect(),
    }
}

/// Reads one group back inside a mutation, so responses carry stored ids.
pub fn read_group(
    conn: &Connection,
    subsystem: SubsystemId,
    group: UpstreamGroupId,
) -> Result<UpstreamGroupConfig, StoreError> {
    subsystems::get(conn, subsystem)?
        .upstream_groups
        .into_iter()
        .find(|candidate| candidate.id == group)
        .ok_or_else(|| StoreError::NotFound(format!("upstream {}", group.0)))
}

// ---- requests -------------------------------------------------------------

/// Body of `POST` on a subsystem's upstreams (with endpoints) and `PUT` (the
/// group's name and policy only). Absent policy sections take their defaults.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpstreamRequest {
    name: String,
    #[serde(default)]
    lb: LbRequest,
    failure_policy: Option<String>,
    #[serde(default)]
    health: HealthRequest,
    #[serde(default)]
    passive: PassiveRequest,
    #[serde(default)]
    retry_budget: RetryBudgetRequest,
    /// Required when creating; not allowed when updating.
    endpoints: Option<Vec<EndpointRequest>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LbRequest {
    algorithm: Option<String>,
    hash_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HealthRequest {
    check: Option<String>,
    path: Option<String>,
    expected_status: Option<[u16; 2]>,
    interval_ms: Option<u64>,
    timeout_ms: Option<u64>,
    rise: Option<u32>,
    fall: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PassiveRequest {
    consecutive_failures: Option<u32>,
    base_ejection_ms: Option<u64>,
    max_ejection_ms: Option<u64>,
    max_ejection_percent: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryBudgetRequest {
    percent: Option<u32>,
    min_per_sec: Option<u32>,
}

/// One endpoint in a request, also the body of the endpoint resource.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EndpointRequest {
    addr: String,
    weight: Option<u32>,
    state: Option<String>,
}

/// A validated upstream request.
pub struct ParsedUpstream {
    pub fields: UpstreamFields,
    /// Present when the request carried endpoints.
    pub endpoints: Option<Vec<EndpointFields>>,
}

impl UpstreamRequest {
    /// Validates everything, reporting every bad field at once. `creating`
    /// requires endpoints; an update must not carry any.
    pub fn parse(self, creating: bool) -> AppResult<ParsedUpstream> {
        let mut errors = FieldErrors::default();
        let name = self.name.trim().to_string();
        if name.is_empty() || name.chars().count() > MAX_NAME_LEN {
            errors.add("name", "must be 1 to 63 characters");
        }
        let policy = parse_policy(&self, &mut errors);
        let endpoints = match (creating, self.endpoints) {
            (true, Some(list)) => parse_endpoint_list(&list, &mut errors),
            (true, None) => {
                errors.add(
                    "endpoints",
                    "is required: a group needs at least one endpoint",
                );
                None
            }
            (false, Some(_)) => {
                errors.add(
                    "endpoints",
                    "cannot be changed here; use the endpoint resources",
                );
                None
            }
            (false, None) => None,
        };
        errors.finish()?;
        let Some(policy) = policy else {
            unreachable!("finish() fails when the policy is invalid");
        };
        Ok(ParsedUpstream {
            fields: UpstreamFields { name, policy },
            endpoints,
        })
    }
}

fn parse_policy(request: &UpstreamRequest, errors: &mut FieldErrors) -> Option<GroupPolicy> {
    let algorithm = errors.check(LbAlgorithm::parse(
        request.lb.algorithm.as_deref().unwrap_or("round_robin"),
    ));
    let hash_key = errors.check(HashKey::parse(
        request.lb.hash_key.as_deref().unwrap_or("client_ip"),
    ));
    let failure = errors.check(FailurePolicy::parse(
        request.failure_policy.as_deref().unwrap_or("fail_open"),
    ));
    let health = parse_health(&request.health, errors);
    let passive = parse_passive(&request.passive, errors);
    let budget = parse_retry_budget(&request.retry_budget, errors);
    Some(GroupPolicy {
        lb: LbPolicy {
            algorithm: algorithm?,
            hash_key: hash_key?,
        },
        failure: failure?,
        health: health?,
        passive: passive?,
        retry_budget: budget?,
    })
}

fn parse_health(request: &HealthRequest, errors: &mut FieldErrors) -> Option<HealthCheck> {
    let defaults = HealthCheck::default();
    let [status_min, status_max] = request
        .expected_status
        .unwrap_or([defaults.status_min, defaults.status_max]);
    let kind = errors.check(HealthKind::parse(
        request.check.as_deref().unwrap_or("none"),
    ));
    let health = HealthCheck {
        kind: kind?,
        path: request.path.clone().unwrap_or(defaults.path),
        status_min,
        status_max,
        interval: request
            .interval_ms
            .map_or(defaults.interval, Duration::from_millis),
        timeout: request
            .timeout_ms
            .map_or(defaults.timeout, Duration::from_millis),
        rise: request.rise.unwrap_or(defaults.rise),
        fall: request.fall.unwrap_or(defaults.fall),
    };
    errors.check(health.validate())?;
    Some(health)
}

fn parse_passive(request: &PassiveRequest, errors: &mut FieldErrors) -> Option<PassiveHealth> {
    let defaults = PassiveHealth::default();
    let passive = PassiveHealth {
        consecutive_failures: request
            .consecutive_failures
            .unwrap_or(defaults.consecutive_failures),
        base_ejection: request
            .base_ejection_ms
            .map_or(defaults.base_ejection, Duration::from_millis),
        max_ejection: request
            .max_ejection_ms
            .map_or(defaults.max_ejection, Duration::from_millis),
        max_ejection_percent: request
            .max_ejection_percent
            .unwrap_or(defaults.max_ejection_percent),
    };
    errors.check(passive.validate())?;
    Some(passive)
}

fn parse_retry_budget(
    request: &RetryBudgetRequest,
    errors: &mut FieldErrors,
) -> Option<RetryBudget> {
    let defaults = RetryBudget::default();
    let budget = RetryBudget {
        percent: request.percent.unwrap_or(defaults.percent),
        min_per_sec: request.min_per_sec.unwrap_or(defaults.min_per_sec),
    };
    errors.check(budget.validate())?;
    Some(budget)
}

fn parse_endpoint_list(
    list: &[EndpointRequest],
    errors: &mut FieldErrors,
) -> Option<Vec<EndpointFields>> {
    if list.is_empty() || list.len() > MAX_ENDPOINTS_PER_GROUP {
        errors.add("endpoints", "a group needs between 1 and 64 endpoints");
        return None;
    }
    let parsed: Vec<_> = list
        .iter()
        .enumerate()
        .map(|(index, endpoint)| parse_endpoint(endpoint, &format!("endpoints.{index}"), errors))
        .collect();
    parsed.into_iter().collect()
}

/// Validates one endpoint; `prefix` is the field path to report errors under
/// (empty for the endpoint resource itself).
pub fn parse_endpoint(
    request: &EndpointRequest,
    prefix: &str,
    errors: &mut FieldErrors,
) -> Option<EndpointFields> {
    let field = |name: &str| {
        if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        }
    };
    let rename = |err: FieldError| FieldError::new(&field(&err.field), err.message);
    let addr = errors.check(EndpointAddr::parse(&request.addr).map_err(rename));
    let weight = errors.check(parse_weight(&field("weight"), request.weight.unwrap_or(1)));
    let state = errors
        .check(EndpointState::parse(request.state.as_deref().unwrap_or("enabled")).map_err(rename));
    Some(EndpointFields {
        addr: addr?,
        weight: weight?,
        state: state?,
    })
}
