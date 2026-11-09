use fast_proxy_model::FieldError;
use rusqlite::ffi;
use thiserror::Error;

/// Why a store operation failed. The admin layer maps these to
/// 404, 409, 400, and 500 responses.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The referenced row does not exist.
    #[error("{0} not found")]
    NotFound(String),
    /// The change collides with existing data (uniqueness, or a row still in use).
    #[error("{0}")]
    Conflict(String),
    /// The input breaks a rule; each entry names the offending field.
    #[error("invalid input: {}", join_fields(.0))]
    Invalid(Vec<FieldError>),
    /// The database file belongs to a different schema version.
    #[error("database schema version {found} is not supported (this build uses {expected})")]
    IncompatibleSchema { found: i64, expected: i64 },
    /// Anything unexpected: I/O, SQLite, corrupt stored data, a stopped store thread.
    #[error("storage error: {0}")]
    Internal(String),
}

fn join_fields(fields: &[FieldError]) -> String {
    fields
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

impl StoreError {
    /// A single-field validation error.
    pub fn invalid_field(field: &str, message: &str) -> Self {
        Self::Invalid(vec![FieldError::new(field, message)])
    }
}

impl From<FieldError> for StoreError {
    fn from(err: FieldError) -> Self {
        Self::Invalid(vec![err])
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(failure, message) = &err {
            let detail = message.as_deref().unwrap_or("constraint violated");
            match failure.extended_code {
                ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
                    return Self::Conflict(format!("already exists ({detail})"));
                }
                ffi::SQLITE_CONSTRAINT_FOREIGNKEY => {
                    return Self::Conflict(format!("still referenced by other data ({detail})"));
                }
                _ => {}
            }
        }
        Self::Internal(err.to_string())
    }
}

impl From<std::io::Error> for StoreError {
    fn from(err: std::io::Error) -> Self {
        Self::Internal(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_lists_every_field() {
        let err = StoreError::Invalid(vec![
            FieldError::new("name", "bad"),
            FieldError::new("addr", "worse"),
        ]);
        assert_eq!(err.to_string(), "invalid input: name: bad; addr: worse");
    }

    #[test]
    fn a_single_field_error_converts_to_invalid() {
        let err = StoreError::from(FieldError::new("prefix", "bad"));
        assert!(matches!(err, StoreError::Invalid(fields) if fields[0].field == "prefix"));
    }

    #[test]
    fn other_sqlite_errors_are_internal() {
        let err = StoreError::from(rusqlite::Error::QueryReturnedNoRows);
        assert!(matches!(err, StoreError::Internal(_)));
    }
}
