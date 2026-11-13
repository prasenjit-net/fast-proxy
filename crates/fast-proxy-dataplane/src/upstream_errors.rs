//! Classifying how an upstream attempt went: transport errors by kind, and
//! response statuses by whether they say the endpoint is unwell.

use std::error::Error;
use std::io;

use http::StatusCode;

use fast_proxy_sampling::FailureKind;
use fast_proxy_telemetry::{UpstreamErrorKind, UpstreamOutcome};

/// A 502, 503 or 504 from the upstream says the endpoint is unwell; every
/// other status, 500 included, is the application talking.
pub(crate) fn outcome_of_status(status: StatusCode) -> UpstreamOutcome {
    match status {
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT => {
            UpstreamOutcome::BadStatus
        }
        _ => UpstreamOutcome::Success,
    }
}

pub(crate) fn outcome_of_error(kind: UpstreamErrorKind) -> UpstreamOutcome {
    match kind {
        UpstreamErrorKind::Connect => UpstreamOutcome::Connect,
        UpstreamErrorKind::Timeout => UpstreamOutcome::Timeout,
        UpstreamErrorKind::Other => UpstreamOutcome::Other,
    }
}

/// Classifies a failed upstream request: anything caused by a timeout is a
/// timeout (504); otherwise a failure to connect is `Connect` and the rest
/// is `Other` (both 502).
pub(crate) fn kind_for(is_connect: bool, err: &(dyn Error + 'static)) -> UpstreamErrorKind {
    if has_timeout_cause(err) {
        UpstreamErrorKind::Timeout
    } else if is_connect {
        UpstreamErrorKind::Connect
    } else {
        UpstreamErrorKind::Other
    }
}

/// How a failed attempt showed itself, for the sampler: finer than the
/// metrics' three kinds. `is_connect` is the client's own judgement.
pub(crate) fn failure_of(
    is_connect: bool,
    kind: UpstreamErrorKind,
    err: &(dyn Error + 'static),
) -> FailureKind {
    match kind {
        UpstreamErrorKind::Timeout if is_connect => FailureKind::ConnectTimeout,
        UpstreamErrorKind::Timeout => FailureKind::ResponseTimeout,
        UpstreamErrorKind::Connect if is_tls_failure(err) => FailureKind::TlsHandshake,
        UpstreamErrorKind::Connect => FailureKind::ConnectRefused,
        UpstreamErrorKind::Other => FailureKind::ConnectionReset,
    }
}

/// Whether a rustls error is anywhere in the chain (the handshake with the
/// upstream failed), including as the payload of an `io::Error`, which does
/// not list its payload among its sources.
fn is_tls_failure(err: &(dyn Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(cause) = current {
        if cause.downcast_ref::<rustls::Error>().is_some() {
            return true;
        }
        if let Some(io_err) = cause.downcast_ref::<io::Error>() {
            if io_err
                .get_ref()
                .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some())
            {
                return true;
            }
        }
        current = cause.source();
    }
    false
}

fn has_timeout_cause(err: &(dyn Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(cause) = current {
        if let Some(io_err) = cause.downcast_ref::<io::Error>() {
            if io_err.kind() == io::ErrorKind::TimedOut {
                return true;
            }
        }
        current = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Wrapper(Box<dyn Error + Send + Sync>);

    impl fmt::Display for Wrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "wrapper")
        }
    }

    impl Error for Wrapper {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn a_timed_out_cause_anywhere_in_the_chain_is_a_timeout() {
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "deadline");
        let nested = Wrapper(Box::new(Wrapper(Box::new(timed_out))));
        assert_eq!(kind_for(false, &nested), UpstreamErrorKind::Timeout);
        // A connect that timed out is still a timeout, not a connect error.
        assert_eq!(kind_for(true, &nested), UpstreamErrorKind::Timeout);
    }

    #[test]
    fn a_failed_connect_is_a_connect_error() {
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            kind_for(true, &Wrapper(Box::new(refused))),
            UpstreamErrorKind::Connect
        );
    }

    #[test]
    fn other_failures_are_other() {
        assert_eq!(
            kind_for(false, &io::Error::other("boom")),
            UpstreamErrorKind::Other
        );
    }

    #[test]
    fn every_kind_of_failure_gets_its_own_name_for_the_sampler() {
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "deadline");
        let tls = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::General("bad certificate".into()),
        );
        let kinds = |is_connect, err: &(dyn Error + 'static)| {
            failure_of(is_connect, kind_for(is_connect, err), err)
        };
        assert_eq!(
            kinds(true, &Wrapper(Box::new(refused))),
            FailureKind::ConnectRefused
        );
        assert_eq!(
            kinds(true, &Wrapper(Box::new(timed_out))),
            FailureKind::ConnectTimeout
        );
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "deadline");
        assert_eq!(
            kinds(false, &Wrapper(Box::new(timed_out))),
            FailureKind::ResponseTimeout
        );
        assert_eq!(
            kinds(true, &Wrapper(Box::new(tls))),
            FailureKind::TlsHandshake
        );
        assert_eq!(
            kinds(false, &io::Error::other("boom")),
            FailureKind::ConnectionReset
        );
    }

    #[test]
    fn a_tls_error_is_found_whether_bare_or_inside_an_io_error() {
        assert!(is_tls_failure(&rustls::Error::General("x".into())));
        let inside = io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::General("x".into()),
        );
        assert!(is_tls_failure(&inside));
        assert!(!is_tls_failure(&io::Error::other("not tls")));
    }

    #[test]
    fn only_502_503_and_504_count_against_an_endpoint() {
        for status in [502, 503, 504] {
            let status = StatusCode::from_u16(status).unwrap();
            assert_eq!(outcome_of_status(status), UpstreamOutcome::BadStatus);
        }
        for status in [200, 301, 404, 429, 500, 501] {
            let status = StatusCode::from_u16(status).unwrap();
            assert_eq!(
                outcome_of_status(status),
                UpstreamOutcome::Success,
                "{status}"
            );
        }
    }

    #[test]
    fn error_kinds_map_to_their_outcomes() {
        assert_eq!(
            outcome_of_error(UpstreamErrorKind::Connect),
            UpstreamOutcome::Connect
        );
        assert_eq!(
            outcome_of_error(UpstreamErrorKind::Timeout),
            UpstreamOutcome::Timeout
        );
        assert_eq!(
            outcome_of_error(UpstreamErrorKind::Other),
            UpstreamOutcome::Other
        );
    }
}
