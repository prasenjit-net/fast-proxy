//! Metrics of the process limits (M7, ADR 0029). The only label is the fixed
//! reason a limit refused something, never an address.

use prometheus::{IntCounter, IntCounterVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

pub(crate) struct LimitsMetrics {
    connections_refused: IntCounterVec,
    client_timeouts: IntCounterVec,
    requests_refused: IntCounterVec,
    rate_limited: IntCounterVec,
    rate_limit_table_full: IntCounter,
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
        let client_timeouts = IntCounterVec::new(
            Opts::new(
                "fastproxy_client_timeouts_total",
                "Client connections and requests ended because the client made no progress",
            ),
            &["kind"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(client_timeouts.clone()))
            .expect(REGISTRATION);
        let requests_refused = IntCounterVec::new(
            Opts::new(
                "fastproxy_requests_refused_total",
                "Requests the proxy answered itself because one of its limits refused them",
            ),
            &["reason"],
        )
        .expect(REGISTRATION);
        let rate_limited = IntCounterVec::new(
            Opts::new(
                "fastproxy_rate_limited_total",
                "Requests answered 429 by a rate-limit policy, by subsystem and route",
            ),
            &["subsystem", "route"],
        )
        .expect(REGISTRATION);
        let rate_limit_table_full = IntCounter::new(
            "fastproxy_rate_limit_table_full_total",
            "New clients let through without a check because the rate-limit key table was full",
        )
        .expect(REGISTRATION);
        for metric in [
            Box::new(requests_refused.clone()) as Box<dyn prometheus::core::Collector>,
            Box::new(rate_limited.clone()),
            Box::new(rate_limit_table_full.clone()),
        ] {
            registry.register(metric).expect(REGISTRATION);
        }
        Self {
            connections_refused,
            client_timeouts,
            requests_refused,
            rate_limited,
            rate_limit_table_full,
        }
    }

    pub(crate) fn request_refused(&self, reason: &'static str) {
        self.requests_refused.with_label_values(&[reason]).inc();
    }

    pub(crate) fn rate_limited(&self, subsystem: &str, route: &str) {
        self.rate_limited
            .with_label_values(&[subsystem, route])
            .inc();
    }

    pub(crate) fn rate_limit_table_full(&self) {
        self.rate_limit_table_full.inc();
    }

    pub(crate) fn client_timeout(&self, kind: &'static str) {
        self.client_timeouts.with_label_values(&[kind]).inc();
    }

    pub(crate) fn connection_refused(&self, reason: &'static str) {
        self.connections_refused.with_label_values(&[reason]).inc();
    }
}
