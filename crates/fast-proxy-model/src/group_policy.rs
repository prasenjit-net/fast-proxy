use crate::bounds::check_range;
use crate::{
    FailurePolicy, FieldError, HealthCheck, LbPolicy, PassiveHealth, RetryBudget, UpstreamTls,
};

/// The highest per-endpoint request cap a group may set.
pub const MAX_REQUESTS_PER_ENDPOINT: u32 = 100_000;

/// Everything configurable about how a group chooses and protects endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPolicy {
    pub lb: LbPolicy,
    pub failure: FailurePolicy,
    pub health: HealthCheck,
    pub passive: PassiveHealth,
    pub retry_budget: RetryBudget,
    pub tls: UpstreamTls,
    /// The most requests one endpoint may have in flight; an endpoint at its cap
    /// is not selected (ADR 0031). `None` is unlimited.
    pub max_requests: Option<u32>,
}

impl GroupPolicy {
    /// Validates every part, returning the first failure.
    pub fn validate(&self) -> Result<(), FieldError> {
        self.health.validate()?;
        self.passive.validate()?;
        self.retry_budget.validate()?;
        self.tls.validate()?;
        self.max_requests.map_or(Ok(()), check_max_requests)
    }
}

/// Fails unless `cap` is a usable per-endpoint request cap.
pub fn check_max_requests(cap: u32) -> Result<(), FieldError> {
    check_range("maxRequests", cap, 1, MAX_REQUESTS_PER_ENDPOINT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_policy_is_valid() {
        assert!(GroupPolicy::default().validate().is_ok());
    }

    #[test]
    fn a_bad_part_is_reported_with_its_field() {
        let mut policy = GroupPolicy::default();
        policy.passive.max_ejection_percent = 0;
        assert_eq!(
            policy.validate().unwrap_err().field,
            "passive.maxEjectionPercent"
        );
        let mut policy = GroupPolicy::default();
        policy.retry_budget.percent = 200;
        assert_eq!(policy.validate().unwrap_err().field, "retryBudget.percent");
    }

    #[test]
    fn a_request_cap_is_optional_but_bounded() {
        let mut policy = GroupPolicy::default();
        assert_eq!(policy.max_requests, None, "unlimited by default");
        for (cap, ok) in [(1, true), (MAX_REQUESTS_PER_ENDPOINT, true), (0, false)] {
            policy.max_requests = Some(cap);
            assert_eq!(policy.validate().is_ok(), ok, "{cap}");
        }
        policy.max_requests = Some(MAX_REQUESTS_PER_ENDPOINT + 1);
        assert_eq!(policy.validate().unwrap_err().field, "maxRequests");
    }
}
