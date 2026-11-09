//! When a failed attempt may be tried again on another endpoint (ADR 0008).
//! Pure: the data plane feeds in what happened and gets back a verdict.

use fast_proxy_model::RetryPolicy;
use fast_proxy_telemetry::RetryReason;
use http::Method;

/// How an attempt failed, from the point of view of retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptFailure {
    /// The connection could not be established: nothing was sent.
    Connect,
    /// No response head arrived in time.
    Timeout,
    /// The connection broke after the request was handed over.
    Reset,
    /// The upstream answered 502, 503 or 504.
    BadStatus,
}

/// Methods whose repetition has no further effect (RFC 9110, 9.2.2).
fn is_idempotent(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::PUT | Method::DELETE | Method::TRACE
    )
}

/// Why the request should be retried, or `None` if it must not be.
///
/// - Never past `max_attempts`, and never unless the whole body can still be
///   sent (`replayable`).
/// - A connect failure is retried for any method: the request never left.
/// - Everything else could have reached the upstream, so only idempotent
///   methods are retried; timeouts only when the route opts in.
pub(crate) fn retry_reason(
    policy: &RetryPolicy,
    method: &Method,
    failure: AttemptFailure,
    replayable: bool,
    attempts_made: u32,
) -> Option<RetryReason> {
    if attempts_made >= policy.max_attempts || !replayable {
        return None;
    }
    let idempotent = is_idempotent(method);
    match failure {
        AttemptFailure::Connect => Some(RetryReason::Connect),
        AttemptFailure::Timeout if idempotent && policy.on_timeout => Some(RetryReason::Timeout),
        AttemptFailure::Reset if idempotent => Some(RetryReason::Reset),
        AttemptFailure::BadStatus if idempotent && policy.idempotent_on_5xx => {
            Some(RetryReason::BadStatus)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: 2,
            idempotent_on_5xx: true,
            on_timeout: false,
        }
    }

    fn reason(method: Method, failure: AttemptFailure) -> Option<RetryReason> {
        retry_reason(&policy(), &method, failure, true, 1)
    }

    #[test]
    fn a_connect_failure_is_retried_for_every_method() {
        for method in [Method::GET, Method::POST, Method::PATCH, Method::PUT] {
            assert_eq!(
                reason(method, AttemptFailure::Connect),
                Some(RetryReason::Connect)
            );
        }
    }

    #[test]
    fn only_idempotent_methods_are_retried_after_the_request_may_have_arrived() {
        for method in [
            Method::GET,
            Method::HEAD,
            Method::OPTIONS,
            Method::PUT,
            Method::DELETE,
            Method::TRACE,
        ] {
            assert_eq!(
                reason(method.clone(), AttemptFailure::Reset),
                Some(RetryReason::Reset),
                "{method}"
            );
            assert_eq!(
                reason(method.clone(), AttemptFailure::BadStatus),
                Some(RetryReason::BadStatus),
                "{method}"
            );
        }
        for method in [Method::POST, Method::PATCH, Method::CONNECT] {
            assert_eq!(
                reason(method.clone(), AttemptFailure::Reset),
                None,
                "{method}"
            );
            assert_eq!(
                reason(method.clone(), AttemptFailure::BadStatus),
                None,
                "{method}"
            );
        }
    }

    #[test]
    fn timeouts_are_retried_only_on_opt_in_and_only_when_idempotent() {
        assert_eq!(reason(Method::GET, AttemptFailure::Timeout), None);
        let opted_in = RetryPolicy {
            on_timeout: true,
            ..policy()
        };
        assert_eq!(
            retry_reason(&opted_in, &Method::GET, AttemptFailure::Timeout, true, 1),
            Some(RetryReason::Timeout)
        );
        assert_eq!(
            retry_reason(&opted_in, &Method::POST, AttemptFailure::Timeout, true, 1),
            None
        );
    }

    #[test]
    fn the_5xx_rule_can_be_switched_off() {
        let off = RetryPolicy {
            idempotent_on_5xx: false,
            ..policy()
        };
        assert_eq!(
            retry_reason(&off, &Method::GET, AttemptFailure::BadStatus, true, 1),
            None
        );
        assert!(retry_reason(&off, &Method::GET, AttemptFailure::Connect, true, 1).is_some());
    }

    #[test]
    fn the_attempt_limit_and_an_unreplayable_body_stop_every_retry() {
        for failure in [
            AttemptFailure::Connect,
            AttemptFailure::Reset,
            AttemptFailure::BadStatus,
        ] {
            assert_eq!(
                retry_reason(&policy(), &Method::GET, failure, true, 2),
                None
            );
            assert_eq!(
                retry_reason(&policy(), &Method::GET, failure, false, 1),
                None
            );
        }
        let never = RetryPolicy {
            max_attempts: 1,
            ..policy()
        };
        assert_eq!(
            retry_reason(&never, &Method::GET, AttemptFailure::Connect, true, 1),
            None
        );
        let more = RetryPolicy {
            max_attempts: 3,
            ..policy()
        };
        assert!(retry_reason(&more, &Method::GET, AttemptFailure::Connect, true, 2).is_some());
    }
}
