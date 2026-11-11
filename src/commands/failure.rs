//! How a command ends badly, and the exit code that says so: 1 when the proxy
//! refused or validation failed, 2 for usage and connection errors (ADR 0019).

use std::process::ExitCode;

#[derive(Debug, PartialEq, Eq)]
pub enum Failure {
    /// The proxy answered "no", or what was checked is not valid.
    Refused(String),
    /// The proxy could not be reached.
    Unreachable(String),
    /// The command was used wrongly (a missing file, a bad value).
    Usage(String),
}

impl Failure {
    pub fn exit_code(&self) -> ExitCode {
        match self {
            Self::Refused(_) => ExitCode::from(1),
            Self::Unreachable(_) | Self::Usage(_) => ExitCode::from(2),
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Refused(text) | Self::Unreachable(text) | Self::Usage(text) => text,
        }
    }
}

/// What a command produces: the text to print, or why it failed.
pub type Outcome = Result<String, Failure>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_exit_with_1_and_everything_else_with_2() {
        assert_eq!(Failure::Refused("x".into()).exit_code(), ExitCode::from(1));
        assert_eq!(
            Failure::Unreachable("x".into()).exit_code(),
            ExitCode::from(2)
        );
        assert_eq!(Failure::Usage("x".into()).exit_code(), ExitCode::from(2));
        assert_eq!(Failure::Usage("why".into()).message(), "why");
    }
}
