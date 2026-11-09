use std::time::Duration;

use crate::FieldError;

const MIN_MS: u64 = 100;
const MAX_MS: u64 = 600_000;

/// Validates a timeout in milliseconds (100 ms to 10 minutes) for `field`.
pub fn parse_timeout_ms(field: &str, millis: u64) -> Result<Duration, FieldError> {
    if !(MIN_MS..=MAX_MS).contains(&millis) {
        return Err(FieldError::new(
            field,
            "must be between 100 ms and 600000 ms (10 minutes)",
        ));
    }
    Ok(Duration::from_millis(millis))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_inclusive_bounds() {
        assert_eq!(
            parse_timeout_ms("connect_timeout_ms", 100).unwrap(),
            Duration::from_millis(100)
        );
        assert!(parse_timeout_ms("connect_timeout_ms", 600_000).is_ok());
    }

    #[test]
    fn rejects_values_outside_the_bounds_naming_the_field() {
        for millis in [0, 99, 600_001, u64::MAX] {
            let err = parse_timeout_ms("response_timeout_ms", millis).unwrap_err();
            assert_eq!(err.field, "response_timeout_ms", "{millis}");
        }
    }
}
