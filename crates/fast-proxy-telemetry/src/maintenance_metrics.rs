//! Maintenance metrics. Labels come from configuration names only: the scope
//! (`global`, `subsystem`, `route`), the subsystem name and the route prefix,
//! with `_none` where one does not apply, so cardinality is bounded by the
//! configuration.

use prometheus::{IntCounterVec, IntGaugeVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// The name of the counter whose per-subsystem sum the traffic sampler
/// subtracts from the 5xx count: maintenance answers are intended, not errors.
pub(crate) const RESPONSES_METRIC: &str = "fastproxy_maintenance_responses_total";

/// One active scope, for the `fastproxy_maintenance_active` gauge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceReading {
    pub scope: &'static str,
    pub subsystem: String,
    pub route: String,
}

pub(crate) struct MaintenanceMetrics {
    active: IntGaugeVec,
    responses: IntCounterVec,
    bypasses: IntCounterVec,
}

impl MaintenanceMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        let active = IntGaugeVec::new(
            Opts::new(
                "fastproxy_maintenance_active",
                "1 while a maintenance scope is on, by scope, subsystem and route",
            ),
            &["scope", "subsystem", "route"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(active.clone()))
            .expect(REGISTRATION);
        let responses = IntCounterVec::new(
            Opts::new(
                RESPONSES_METRIC,
                "Requests answered with the maintenance response, by the scope that answered \
                 and the subsystem and route the request matched",
            ),
            &["scope", "subsystem", "route"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(responses.clone()))
            .expect(REGISTRATION);
        let bypasses = IntCounterVec::new(
            Opts::new(
                "fastproxy_maintenance_bypass_total",
                "Requests that passed through a maintenance scope by a bypass rule",
            ),
            &["scope", "subsystem"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(bypasses.clone()))
            .expect(REGISTRATION);
        Self {
            active,
            responses,
            bypasses,
        }
    }

    pub(crate) fn response(&self, scope: &str, subsystem: &str, route: &str) {
        self.responses
            .with_label_values(&[scope, subsystem, route])
            .inc();
    }

    pub(crate) fn bypass(&self, scope: &str, subsystem: &str) {
        self.bypasses.with_label_values(&[scope, subsystem]).inc();
    }

    /// Replaces the gauge: exactly the given scopes read 1, and a scope that
    /// ended disappears.
    pub(crate) fn set_active(&self, readings: &[MaintenanceReading]) {
        self.active.reset();
        for reading in readings {
            self.active
                .with_label_values(&[reading.scope, &reading.subsystem, &reading.route])
                .set(1);
        }
    }
}
