use crate::bounds::check_range;
use crate::{EndpointAddr, FieldError};

/// Most endpoints one upstream group may hold; bounds per-request selection
/// and per-compile cost.
pub const MAX_ENDPOINTS_PER_GROUP: usize = 64;

const MIN_WEIGHT: u32 = 1;
const MAX_WEIGHT: u32 = 1000;

/// Database id of an endpoint. Stable across edits and rollbacks, so runtime
/// state can be keyed by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EndpointId(pub i64);

/// What the operator has decided about an endpoint. Only `Enabled` receives
/// new requests; the other two let in-flight requests finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointState {
    /// Takes new requests.
    Enabled,
    /// Leaving soon: no new requests, in-flight ones finish.
    Draining,
    /// Parked: no new requests, in-flight ones finish.
    Disabled,
}

impl EndpointState {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Draining => "draining",
            Self::Disabled => "disabled",
        }
    }

    /// Parses the stored and API spelling.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "enabled" => Ok(Self::Enabled),
            "draining" => Ok(Self::Draining),
            "disabled" => Ok(Self::Disabled),
            _ => Err(FieldError::new(
                "state",
                "must be enabled, draining or disabled",
            )),
        }
    }
}

/// Validates an endpoint weight (1 to 1000; use `Disabled` instead of 0).
pub fn parse_weight(field: &str, weight: u32) -> Result<u32, FieldError> {
    check_range(field, weight, MIN_WEIGHT, MAX_WEIGHT)?;
    Ok(weight)
}

/// One backend address in an upstream group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointConfig {
    pub id: EndpointId,
    pub addr: EndpointAddr,
    pub weight: u32,
    pub state: EndpointState,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_accept_the_bounds_and_reject_outside() {
        assert_eq!(parse_weight("weight", 1).unwrap(), 1);
        assert_eq!(parse_weight("weight", 1000).unwrap(), 1000);
        for bad in [0, 1001] {
            assert_eq!(
                parse_weight("endpoints.0.weight", bad).unwrap_err().field,
                "endpoints.0.weight"
            );
        }
    }

    #[test]
    fn states_round_trip_and_reject_unknown() {
        for state in [
            EndpointState::Enabled,
            EndpointState::Draining,
            EndpointState::Disabled,
        ] {
            assert_eq!(EndpointState::parse(state.as_str()).unwrap(), state);
        }
        assert_eq!(EndpointState::parse("paused").unwrap_err().field, "state");
    }
}
