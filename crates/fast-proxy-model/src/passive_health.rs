use std::time::Duration;

use crate::bounds::{check_millis, check_range};
use crate::FieldError;

const MIN_EJECTION_MS: u64 = 100;
const MAX_EJECTION_MS: u64 = 3_600_000;

/// Passive outlier-ejection settings of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassiveHealth {
    /// Consecutive failures that eject an endpoint; 0 turns ejection off.
    pub consecutive_failures: u32,
    /// First ejection length; doubles on each repeat up to `max_ejection`.
    pub base_ejection: Duration,
    pub max_ejection: Duration,
    /// Never eject more than this share of a group, in percent.
    pub max_ejection_percent: u32,
}

impl Default for PassiveHealth {
    fn default() -> Self {
        Self {
            consecutive_failures: 3,
            base_ejection: Duration::from_secs(10),
            max_ejection: Duration::from_secs(300),
            max_ejection_percent: 50,
        }
    }
}

impl PassiveHealth {
    /// Checks every bound; errors name the field in the API's dotted spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range(
            "passive.consecutiveFailures",
            self.consecutive_failures,
            0,
            100,
        )?;
        check_millis(
            "passive.baseEjectionMs",
            self.base_ejection,
            MIN_EJECTION_MS,
            MAX_EJECTION_MS,
        )?;
        check_millis(
            "passive.maxEjectionMs",
            self.max_ejection,
            MIN_EJECTION_MS,
            MAX_EJECTION_MS,
        )?;
        if self.max_ejection < self.base_ejection {
            return Err(FieldError::new(
                "passive.maxEjectionMs",
                "must not be shorter than the base ejection",
            ));
        }
        check_range(
            "passive.maxEjectionPercent",
            self.max_ejection_percent,
            1,
            100,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Case = (&'static str, Box<dyn FnOnce(&mut PassiveHealth)>);

    fn with(change: impl FnOnce(&mut PassiveHealth)) -> Result<(), FieldError> {
        let mut passive = PassiveHealth::default();
        change(&mut passive);
        passive.validate()
    }

    #[test]
    fn defaults_are_valid_and_match_the_documented_values() {
        let passive = PassiveHealth::default();
        assert!(passive.validate().is_ok());
        assert_eq!(passive.consecutive_failures, 3);
        assert_eq!(passive.base_ejection, Duration::from_secs(10));
        assert_eq!(passive.max_ejection, Duration::from_secs(300));
        assert_eq!(passive.max_ejection_percent, 50);
    }

    #[test]
    fn zero_failures_disables_ejection_and_is_valid() {
        assert!(with(|p| p.consecutive_failures = 0).is_ok());
    }

    #[test]
    fn every_bound_is_enforced_with_its_field() {
        let cases: Vec<Case> = vec![
            (
                "passive.consecutiveFailures",
                Box::new(|p| p.consecutive_failures = 101),
            ),
            (
                "passive.baseEjectionMs",
                Box::new(|p| p.base_ejection = Duration::from_millis(99)),
            ),
            (
                "passive.baseEjectionMs",
                Box::new(|p| p.base_ejection = Duration::from_secs(3601)),
            ),
            (
                "passive.maxEjectionMs",
                Box::new(|p| p.max_ejection = Duration::from_secs(3601)),
            ),
            (
                "passive.maxEjectionMs",
                Box::new(|p| p.max_ejection = Duration::from_secs(1)),
            ),
            (
                "passive.maxEjectionPercent",
                Box::new(|p| p.max_ejection_percent = 0),
            ),
            (
                "passive.maxEjectionPercent",
                Box::new(|p| p.max_ejection_percent = 101),
            ),
        ];
        for (field, change) in cases {
            assert_eq!(with(change).unwrap_err().field, field);
        }
    }
}
