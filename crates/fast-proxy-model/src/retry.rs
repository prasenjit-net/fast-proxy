use crate::bounds::check_range;
use crate::FieldError;

/// Per-group cap on retries so a failing group is not hammered by its own
/// retries: at most `percent` of recent requests, never fewer than
/// `min_per_sec` retries per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryBudget {
    pub percent: u32,
    pub min_per_sec: u32,
}

impl Default for RetryBudget {
    fn default() -> Self {
        Self {
            percent: 20,
            min_per_sec: 10,
        }
    }
}

impl RetryBudget {
    /// Checks every bound; errors name the field in the API's dotted spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range("retryBudget.percent", self.percent, 0, 100)?;
        check_range("retryBudget.minPerSec", self.min_per_sec, 0, 1000)
    }
}

/// A route's retry behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first; 1 means never retry.
    pub max_attempts: u32,
    /// Also retry idempotent methods on 502/503/504.
    pub idempotent_on_5xx: bool,
    /// Also retry response timeouts (opt-in).
    pub on_timeout: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            idempotent_on_5xx: true,
            on_timeout: false,
        }
    }
}

impl RetryPolicy {
    /// Checks every bound; errors name the field in the API's camelCase spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range("retry.maxAttempts", self.max_attempts, 1, 5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_documented_values_and_are_valid() {
        let budget = RetryBudget::default();
        assert_eq!((budget.percent, budget.min_per_sec), (20, 10));
        assert!(budget.validate().is_ok());
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_attempts, 2);
        assert!(policy.idempotent_on_5xx && !policy.on_timeout);
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn budget_bounds_name_their_field() {
        let percent = RetryBudget {
            percent: 101,
            ..RetryBudget::default()
        };
        assert_eq!(percent.validate().unwrap_err().field, "retryBudget.percent");
        let rate = RetryBudget {
            min_per_sec: 1001,
            ..RetryBudget::default()
        };
        assert_eq!(rate.validate().unwrap_err().field, "retryBudget.minPerSec");
        assert!(RetryBudget {
            percent: 0,
            min_per_sec: 0
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn attempts_are_one_to_five() {
        for attempts in [1, 5] {
            assert!(RetryPolicy {
                max_attempts: attempts,
                ..RetryPolicy::default()
            }
            .validate()
            .is_ok());
        }
        for attempts in [0, 6] {
            let err = RetryPolicy {
                max_attempts: attempts,
                ..RetryPolicy::default()
            }
            .validate()
            .unwrap_err();
            assert_eq!(err.field, "retry.maxAttempts");
        }
    }
}
