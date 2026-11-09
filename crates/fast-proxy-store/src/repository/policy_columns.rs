//! The `upstream_group` policy columns and their mapping to [`GroupPolicy`],
//! shared by create, update, load and revision restore so the column list
//! lives in one place.

use std::time::Duration;

use fast_proxy_model::{
    FailurePolicy, GroupPolicy, HashKey, HealthCheck, HealthKind, LbAlgorithm, LbPolicy,
    PassiveHealth, RetryBudget,
};
use rusqlite::Row;

use crate::StoreError;

/// Column names in the order [`read`] expects them.
pub const SELECT_LIST: &str = "lb_algorithm, lb_hash_key, failure_policy, health_check, \
     health_path, health_status_min, health_status_max, health_interval_ms, health_timeout_ms, \
     health_rise, health_fall, passive_failures, passive_base_ms, passive_max_ms, \
     passive_max_percent, retry_budget_percent, retry_budget_min_per_sec";

/// Assignments for an UPDATE using the named parameters of [`Bound`].
pub const UPDATE_SET: &str = "lb_algorithm = :lb_algorithm, lb_hash_key = :lb_hash_key, \
     failure_policy = :failure_policy, health_check = :health_check, health_path = :health_path, \
     health_status_min = :health_status_min, health_status_max = :health_status_max, \
     health_interval_ms = :health_interval_ms, health_timeout_ms = :health_timeout_ms, \
     health_rise = :health_rise, health_fall = :health_fall, passive_failures = :passive_failures, \
     passive_base_ms = :passive_base_ms, passive_max_ms = :passive_max_ms, \
     passive_max_percent = :passive_max_percent, retry_budget_percent = :retry_budget_percent, \
     retry_budget_min_per_sec = :retry_budget_min_per_sec";

/// Column list for an INSERT; pair with [`INSERT_VALUES`].
pub const INSERT_COLUMNS: &str = SELECT_LIST;

/// Named placeholders matching [`INSERT_COLUMNS`].
pub const INSERT_VALUES: &str = ":lb_algorithm, :lb_hash_key, :failure_policy, :health_check, \
     :health_path, :health_status_min, :health_status_max, :health_interval_ms, \
     :health_timeout_ms, :health_rise, :health_fall, :passive_failures, :passive_base_ms, \
     :passive_max_ms, :passive_max_percent, :retry_budget_percent, :retry_budget_min_per_sec";

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// A policy flattened into the values bound to the named parameters.
pub struct Bound {
    lb_algorithm: &'static str,
    lb_hash_key: String,
    failure_policy: &'static str,
    health_check: &'static str,
    health_path: String,
    health_status_min: u16,
    health_status_max: u16,
    health_interval_ms: i64,
    health_timeout_ms: i64,
    health_rise: u32,
    health_fall: u32,
    passive_failures: u32,
    passive_base_ms: i64,
    passive_max_ms: i64,
    passive_max_percent: u32,
    retry_budget_percent: u32,
    retry_budget_min_per_sec: u32,
}

impl Bound {
    pub fn new(policy: &GroupPolicy) -> Self {
        Self {
            lb_algorithm: policy.lb.algorithm.as_str(),
            lb_hash_key: policy.lb.hash_key.to_spec(),
            failure_policy: policy.failure.as_str(),
            health_check: policy.health.kind.as_str(),
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

    /// Named parameters for the policy placeholders (callers add their own).
    pub fn named(&self) -> Vec<(&'static str, &dyn rusqlite::ToSql)> {
        vec![
            (":lb_algorithm", &self.lb_algorithm),
            (":lb_hash_key", &self.lb_hash_key),
            (":failure_policy", &self.failure_policy),
            (":health_check", &self.health_check),
            (":health_path", &self.health_path),
            (":health_status_min", &self.health_status_min),
            (":health_status_max", &self.health_status_max),
            (":health_interval_ms", &self.health_interval_ms),
            (":health_timeout_ms", &self.health_timeout_ms),
            (":health_rise", &self.health_rise),
            (":health_fall", &self.health_fall),
            (":passive_failures", &self.passive_failures),
            (":passive_base_ms", &self.passive_base_ms),
            (":passive_max_ms", &self.passive_max_ms),
            (":passive_max_percent", &self.passive_max_percent),
            (":retry_budget_percent", &self.retry_budget_percent),
            (":retry_budget_min_per_sec", &self.retry_budget_min_per_sec),
        ]
    }
}

/// Reads a policy from `row` starting at column `first` (the order of
/// [`SELECT_LIST`]). Stored values that no longer validate are `Internal`.
pub fn read(row: &Row<'_>, first: usize) -> rusqlite::Result<Result<GroupPolicy, StoreError>> {
    let text = |offset: usize| row.get::<_, String>(first + offset);
    let number = |offset: usize| row.get::<_, i64>(first + offset);
    let algorithm = text(0)?;
    let hash_key = text(1)?;
    let failure = text(2)?;
    let kind = text(3)?;
    let path = text(4)?;
    let numbers: Vec<i64> = (5..17).map(number).collect::<Result<_, _>>()?;
    Ok(build(
        &algorithm, &hash_key, &failure, &kind, path, &numbers,
    ))
}

fn build(
    algorithm: &str,
    hash_key: &str,
    failure: &str,
    kind: &str,
    path: String,
    n: &[i64],
) -> Result<GroupPolicy, StoreError> {
    let corrupt = |err: fast_proxy_model::FieldError| {
        StoreError::Internal(format!("stored upstream policy is invalid: {err}"))
    };
    let small = |value: i64| u32::try_from(value).unwrap_or(u32::MAX);
    let status = |value: i64| u16::try_from(value).unwrap_or(u16::MAX);
    let duration = |value: i64| Duration::from_millis(u64::try_from(value).unwrap_or(0));
    let policy = GroupPolicy {
        lb: LbPolicy {
            algorithm: LbAlgorithm::parse(algorithm).map_err(corrupt)?,
            hash_key: HashKey::parse(hash_key).map_err(corrupt)?,
        },
        failure: FailurePolicy::parse(failure).map_err(corrupt)?,
        health: HealthCheck {
            kind: HealthKind::parse(kind).map_err(corrupt)?,
            path,
            status_min: status(n[0]),
            status_max: status(n[1]),
            interval: duration(n[2]),
            timeout: duration(n[3]),
            rise: small(n[4]),
            fall: small(n[5]),
        },
        passive: PassiveHealth {
            consecutive_failures: small(n[6]),
            base_ejection: duration(n[7]),
            max_ejection: duration(n[8]),
            max_ejection_percent: small(n[9]),
        },
        retry_budget: RetryBudget {
            percent: small(n[10]),
            min_per_sec: small(n[11]),
        },
    };
    policy.validate().map_err(corrupt)?;
    Ok(policy)
}
