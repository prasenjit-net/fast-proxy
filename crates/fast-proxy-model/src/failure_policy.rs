use crate::FieldError;

/// What a group does when no endpoint is eligible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FailurePolicy {
    /// "Panic mode": ignore health, still honour admin state.
    #[default]
    FailOpen,
    /// Answer `503 no healthy upstream`.
    FailClosed,
}

impl FailurePolicy {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FailOpen => "fail_open",
            Self::FailClosed => "fail_closed",
        }
    }

    /// Parses the stored and API spelling.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "fail_open" => Ok(Self::FailOpen),
            "fail_closed" => Ok(Self::FailClosed),
            _ => Err(FieldError::new(
                "failurePolicy",
                "must be fail_open or fail_closed",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_defaults_to_fail_open() {
        assert_eq!(FailurePolicy::default(), FailurePolicy::FailOpen);
        for policy in [FailurePolicy::FailOpen, FailurePolicy::FailClosed] {
            assert_eq!(FailurePolicy::parse(policy.as_str()).unwrap(), policy);
        }
        assert_eq!(
            FailurePolicy::parse("x").unwrap_err().field,
            "failurePolicy"
        );
    }
}
