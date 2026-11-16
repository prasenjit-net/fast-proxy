//! Metrics of the process limits (M7, ADR 0029). The only label is the fixed
//! reason a limit refused something, never an address.

use prometheus::{IntCounterVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

pub(crate) struct LimitsMetrics {
    connections_refused: IntCounterVec,
}

impl LimitsMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        let connections_refused = IntCounterVec::new(
            Opts::new(
                "fastproxy_connections_refused_total",
                "Client connections closed when accepted because a connection cap was reached",
            ),
            &["reason"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(connections_refused.clone()))
            .expect(REGISTRATION);
        Self {
            connections_refused,
        }
    }

    pub(crate) fn connection_refused(&self, reason: &'static str) {
        self.connections_refused.with_label_values(&[reason]).inc();
    }
}
