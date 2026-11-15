//! ACME metrics (M4). Labels are fixed enums only: an HTTP-01 challenge
//! request carries a token and a host, and neither may become a label.

use prometheus::{IntCounterVec, IntGauge, Opts, Registry};

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

/// How an ACME order ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcmeOrderOutcome {
    /// A first certificate was stored.
    Issued,
    /// A certificate was replaced in place.
    Renewed,
    /// The CA asked us to wait.
    RateLimited,
    /// The challenge could not be fetched through its public name.
    SelfCheckFailed,
    /// Any other failure.
    Failed,
}

impl AcmeOrderOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Issued => "issued",
            Self::Renewed => "renewed",
            Self::RateLimited => "rate_limited",
            Self::SelfCheckFailed => "self_check_failed",
            Self::Failed => "failed",
        }
    }
}

pub(crate) struct AcmeMetrics {
    challenge_requests: IntCounterVec,
    orders: IntCounterVec,
    lease_held: IntGauge,
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
        let orders = IntCounterVec::new(
            Opts::new(
                "fastproxy_acme_orders_total",
                "ACME orders placed by this instance, by subsystem and how they ended",
            ),
            &["subsystem", "outcome"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(orders.clone()))
            .expect(REGISTRATION);
        let lease_held = IntGauge::new(
            "fastproxy_acme_lease_held",
            "1 while this instance holds the ACME renewal lease",
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(lease_held.clone()))
            .expect(REGISTRATION);
        Self {
            challenge_requests,
            orders,
            lease_held,
        }
    }

    pub(crate) fn order(&self, subsystem: &str, outcome: AcmeOrderOutcome) {
        self.orders
            .with_label_values(&[subsystem, outcome.label()])
            .inc();
    }

    pub(crate) fn set_lease_held(&self, held: bool) {
        self.lease_held.set(i64::from(held));
    }

    pub(crate) fn challenge_request(&self, outcome: AcmeChallengeOutcome) {
        self.challenge_requests
            .with_label_values(&[outcome.label()])
            .inc();
    }
}
