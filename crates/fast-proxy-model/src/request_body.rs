//! The per-route cap on a request body (ADR 0029). Bodies larger than the cap
//! are refused with 413; a route without one uses the process default.

use crate::FieldError;

/// The largest cap a route may set (1 TiB): above it a cap protects nothing.
pub const MAX_REQUEST_BODY_BYTES: u64 = 1 << 40;

/// Fails unless `value` is a usable body cap: at least one byte, at most
/// [`MAX_REQUEST_BODY_BYTES`].
pub fn check_max_body_bytes(field: &str, value: u64) -> Result<(), FieldError> {
    if (1..=MAX_REQUEST_BODY_BYTES).contains(&value) {
        return Ok(());
    }
    Err(FieldError::new(
        field,
        format!("must be between 1 and {MAX_REQUEST_BODY_BYTES} bytes"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_is_bounded_and_names_the_field() {
        assert!(check_max_body_bytes("maxBodyBytes", 1).is_ok());
        assert!(check_max_body_bytes("maxBodyBytes", MAX_REQUEST_BODY_BYTES).is_ok());
        for bad in [0, MAX_REQUEST_BODY_BYTES + 1] {
            assert_eq!(
                check_max_body_bytes("maxBodyBytes", bad).unwrap_err().field,
                "maxBodyBytes"
            );
        }
    }
}
