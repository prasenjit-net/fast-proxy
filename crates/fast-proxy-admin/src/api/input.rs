use fast_proxy_model::FieldError;

use crate::AppError;

use std::collections::HashMap;

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

/// A non-negative integer query parameter, or a field error; `None` when the
/// parameter is absent.
pub fn query_number(
    params: &HashMap<String, String>,
    name: &str,
    errors: &mut FieldErrors,
) -> Option<u64> {
    let raw = params.get(name)?;
    match raw.parse::<u64>() {
        Ok(value) => Some(value),
        Err(_) => {
            errors.add(name, "must be a non-negative whole number");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_number_is_parsed_or_reported_by_name() {
        let mut errors = FieldErrors::default();
        let params: HashMap<String, String> = [("limit", "12"), ("before", "-1"), ("since", "x")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(query_number(&params, "limit", &mut errors), Some(12));
        assert_eq!(query_number(&params, "absent", &mut errors), None);
        assert_eq!(query_number(&params, "before", &mut errors), None);
        assert_eq!(query_number(&params, "since", &mut errors), None);
        let Err(AppError::Validation(fields)) = errors.finish() else {
            panic!("expected validation errors");
        };
        assert_eq!(
            fields.iter().map(|f| f.field.as_str()).collect::<Vec<_>>(),
            ["before", "since"]
        );
    }

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
