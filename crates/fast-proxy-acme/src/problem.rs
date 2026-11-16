//! How a failed order is treated, from the CA's problem type (RFC 8555 section
//! 6.7) or a local failure. The kind decides the backoff (K7).

/// The prefix of every ACME problem type.
const ACME_ERROR_PREFIX: &str = "urn:ietf:params:acme:error:";

/// Problem types that mean the name can never validate as configured (DNS
/// does not point here, CAA forbids the CA, the CA refuses the name), so
/// retrying soon would only spend rate limit.
const PERMANENT_PROBLEMS: [&str; 6] = [
    "badDomain",
    "rejectedIdentifier",
    "unsupportedIdentifier",
    "caa",
    "dns",
    "unauthorized",
];

/// What a failure means for the next attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Worth retrying on the backoff ladder (network, CA busy, self-check).
    Transient,
    /// Will not succeed without a configuration or DNS change: wait long.
    Permanent,
    /// The CA asked us to wait, until `retry_at_ms` when it said so.
    RateLimited { retry_at_ms: Option<i64> },
}

/// Classifies a CA problem by its type URN; `retry_at_ms` is the absolute time
/// from a `Retry-After` header, if the response had one. Unknown and
/// non-ACME types are transient.
pub fn classify_problem(problem_type: &str, retry_at_ms: Option<i64>) -> FailureKind {
    let Some(name) = problem_type.strip_prefix(ACME_ERROR_PREFIX) else {
        return FailureKind::Transient;
    };
    if name == "rateLimited" {
        return FailureKind::RateLimited { retry_at_ms };
    }
    if PERMANENT_PROBLEMS.contains(&name) {
        return FailureKind::Permanent;
    }
    FailureKind::Transient
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urn(name: &str) -> String {
        format!("{ACME_ERROR_PREFIX}{name}")
    }

    #[test]
    fn problems_are_classified_by_their_type() {
        assert_eq!(
            classify_problem(&urn("rateLimited"), Some(5)),
            FailureKind::RateLimited {
                retry_at_ms: Some(5)
            }
        );
        assert_eq!(
            classify_problem(&urn("rateLimited"), None),
            FailureKind::RateLimited { retry_at_ms: None }
        );
        for permanent in PERMANENT_PROBLEMS {
            assert_eq!(
                classify_problem(&urn(permanent), None),
                FailureKind::Permanent,
                "{permanent}"
            );
        }
        for transient in [
            urn("serverInternal"),
            urn("connection"),
            urn("badNonce"),
            "about:blank".to_string(),
            String::new(),
            "rateLimited".to_string(),
        ] {
            assert_eq!(
                classify_problem(&transient, Some(1)),
                FailureKind::Transient,
                "{transient}"
            );
        }
    }
}
