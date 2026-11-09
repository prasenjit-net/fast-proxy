use crate::{FailurePolicy, FieldError, HealthCheck, LbPolicy, PassiveHealth, RetryBudget};

/// Everything configurable about how a group chooses and protects endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroupPolicy {
    pub lb: LbPolicy,
    pub failure: FailurePolicy,
    pub health: HealthCheck,
    pub passive: PassiveHealth,
    pub retry_budget: RetryBudget,
}

impl GroupPolicy {
    /// Validates every part, returning the first failure.
    pub fn validate(&self) -> Result<(), FieldError> {
        self.health.validate()?;
        self.passive.validate()?;
        self.retry_budget.validate()
    }
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
}
