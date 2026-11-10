use std::time::Duration;

use crate::FieldError;

/// Fails with a field-specific message unless `min <= value <= max`.
pub(crate) fn check_range(field: &str, value: u32, min: u32, max: u32) -> Result<(), FieldError> {
    if (min..=max).contains(&value) {
        return Ok(());
    }
    Err(FieldError::new(
        field,
        format!("must be between {min} and {max}"),
    ))
}

/// Fails unless the duration lies within `min..=max` milliseconds.
pub(crate) fn check_millis(
    field: &str,
    value: Duration,
    min: u64,
    max: u64,
) -> Result<(), FieldError> {
    let millis = u64::try_from(value.as_millis()).unwrap_or(u64::MAX);
    if (min..=max).contains(&millis) {
        return Ok(());
    }
    Err(FieldError::new(
        field,
        format!("must be between {min} ms and {max} ms"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_inclusive_and_names_the_field() {
        assert!(check_range("x", 1, 1, 3).is_ok());
        assert!(check_range("x", 3, 1, 3).is_ok());
        for bad in [0, 4] {
            assert_eq!(check_range("x.y", bad, 1, 3).unwrap_err().field, "x.y");
        }
    }

    #[test]
    fn millis_is_inclusive_and_names_the_field() {
        assert!(check_millis("t", Duration::from_millis(100), 100, 200).is_ok());
        assert!(check_millis("t", Duration::from_millis(200), 100, 200).is_ok());
        for bad in [99, 201] {
            let err = check_millis("t", Duration::from_millis(bad), 100, 200).unwrap_err();
            assert_eq!(err.field, "t");
        }
    }
}
