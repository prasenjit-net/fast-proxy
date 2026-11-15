//! ACME metrics (M4). Labels are fixed enums only: an HTTP-01 challenge
//! request carries a token and a host, and neither may become a label.

use prometheus::{IntCounterVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// What happened to a request on the ACME challenge path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcmeChallengeOutcome {
    /// A live token for that host: the proxy answered with the key authorization.
    Answered,
    /// No live token for that host: the request was routed as usual.
    NotAnswered,
}

impl AcmeChallengeOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Answered => "answered",
            Self::NotAnswered => "not_answered",
        }
    }
}

pub(crate) struct AcmeMetrics {
    challenge_requests: IntCounterVec,
}

impl AcmeMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        let challenge_requests = IntCounterVec::new(
            Opts::new(
                "fastproxy_acme_challenge_requests_total",
                "Requests on /.well-known/acme-challenge/, answered by the proxy or routed as usual",
            ),
            &["outcome"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(challenge_requests.clone()))
            .expect(REGISTRATION);
        Self { challenge_requests }
    }

    pub(crate) fn challenge_request(&self, outcome: AcmeChallengeOutcome) {
        self.challenge_requests
            .with_label_values(&[outcome.label()])
            .inc();
    }
}
