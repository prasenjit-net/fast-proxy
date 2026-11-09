use fast_proxy_model::FieldError;

use crate::AppError;

/// Collects every invalid field of a request so the client sees them all
/// at once instead of one per round trip.
#[derive(Default)]
pub struct FieldErrors(Vec<FieldError>);

impl FieldErrors {
    /// Unwraps a parsed value, remembering the error if it failed.
    pub fn check<T>(&mut self, parsed: Result<T, FieldError>) -> Option<T> {
        match parsed {
            Ok(value) => Some(value),
            Err(err) => {
                self.0.push(err);
                None
            }
        }
    }

    /// Records an error directly.
    pub fn add(&mut self, field: &str, message: &str) {
        self.0.push(FieldError::new(field, message));
    }

    /// `Ok` only if no field failed.
    pub fn finish(self) -> Result<(), AppError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(AppError::Validation(self.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_every_failure() {
        let mut errors = FieldErrors::default();
        assert_eq!(errors.check(Ok::<_, FieldError>(1)), Some(1));
        assert_eq!(
            errors.check(Err::<i32, _>(FieldError::new("a", "bad"))),
            None
        );
        errors.add("b", "worse");
        let Err(AppError::Validation(fields)) = errors.finish() else {
            panic!("expected validation errors");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[1].field, "b");
    }

    #[test]
    fn no_failures_is_ok() {
        assert!(FieldErrors::default().finish().is_ok());
    }
}
