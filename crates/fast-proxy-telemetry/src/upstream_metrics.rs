//! Per-endpoint metrics. Label values come only from config names (subsystem
//! and upstream names, endpoint `host:port`), so cardinality is bounded by
//! the configuration; series of removed endpoints are deleted.

use std::collections::HashSet;

use prometheus::core::Collector;
use prometheus::{IntCounterVec, IntGaugeVec, Opts, Registry};

const ENDPOINT_REQUEST_LABELS: [&str; 4] = ["subsystem", "upstream", "endpoint", "outcome"];
const HEALTH_CHECK_LABELS: [&str; 4] = ["subsystem", "upstream", "endpoint", "result"];
const ENDPOINT_LABELS: [&str; 3] = ["subsystem", "upstream", "endpoint"];
const GROUP_LABELS: [&str; 2] = ["subsystem", "upstream"];
const RETRY_LABELS: [&str; 3] = ["subsystem", "upstream", "reason"];
const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// How a request to an endpoint turned out, as far as health is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamOutcome {
    /// Any response other than 502, 503 or 504.
    Success,
    /// The upstream answered 502, 503 or 504.
    BadStatus,
    /// The connection could not be established.
    Connect,
    /// The upstream did not answer in time.
    Timeout,
    /// The connection broke some other way.
    Other,
}

impl UpstreamOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::BadStatus => "bad_status",
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Other => "other",
        }
    }
}

/// Why a request was tried again on another endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryReason {
    /// The connection could not be established.
    Connect,
    /// The response timed out (retried only when the route opts in).
    Timeout,
    /// The connection broke after the request was sent.
    Reset,
    /// The upstream answered 502, 503 or 504.
    BadStatus,
}

impl RetryReason {
    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Reset => "reset",
            Self::BadStatus => "bad_status",
        }
    }
}

/// Identifies one endpoint in metric labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EndpointLabels<'a> {
    pub subsystem: &'a str,
    pub upstream: &'a str,
    pub endpoint: &'a str,
}

/// Counters for upstream requests, ejections and panic-mode picks.
pub(crate) struct UpstreamMetrics {
    requests: IntCounterVec,
    ejections: IntCounterVec,
    panics: IntCounterVec,
    retries: IntCounterVec,
    budget_exhausted: IntCounterVec,
    health_checks: IntCounterVec,
    endpoint_healthy: IntGaugeVec,
    endpoint_in_flight: IntGaugeVec,
}

fn counter(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let counter = IntCounterVec::new(Opts::new(name, help), labels).expect(REGISTRATION);
    registry
        .register(Box::new(counter.clone()))
        .expect(REGISTRATION);
    counter
}

fn gauge(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntGaugeVec {
    let gauge = IntGaugeVec::new(Opts::new(name, help), labels).expect(REGISTRATION);
    registry
        .register(Box::new(gauge.clone()))
        .expect(REGISTRATION);
    gauge
}

impl UpstreamMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        Self {
            requests: counter(
                registry,
                "fastproxy_upstream_requests_total",
                "Requests sent to upstream endpoints, by outcome",
                &ENDPOINT_REQUEST_LABELS,
            ),
            ejections: counter(
                registry,
                "fastproxy_endpoint_ejections_total",
                "Times an endpoint was taken out of rotation after repeated failures",
                &ENDPOINT_LABELS,
            ),
            health_checks: counter(
                registry,
                "fastproxy_health_checks_total",
                "Active health probes, by result",
                &HEALTH_CHECK_LABELS,
            ),
            endpoint_healthy: gauge(
                registry,
                "fastproxy_upstream_endpoint_healthy",
                "1 if the endpoint is healthy and not ejected, else 0",
                &ENDPOINT_LABELS,
            ),
            endpoint_in_flight: gauge(
                registry,
                "fastproxy_upstream_endpoint_in_flight",
                "Requests currently in flight on the endpoint",
                &ENDPOINT_LABELS,
            ),
            retries: counter(
                registry,
                "fastproxy_upstream_retries_total",
                "Requests tried again on another endpoint, by reason",
                &["subsystem", "upstream", "reason"],
            ),
            budget_exhausted: counter(
                registry,
                "fastproxy_retry_budget_exhausted_total",
                "Retries refused because the group's retry budget was spent",
                &GROUP_LABELS,
            ),
            panics: counter(
                registry,
                "fastproxy_upstream_panic_total",
                "Requests served in panic mode (no healthy endpoint, group fails open)",
                &GROUP_LABELS,
            ),
        }
    }

    pub(crate) fn request(&self, endpoint: EndpointLabels<'_>, outcome: UpstreamOutcome) {
        self.requests
            .with_label_values(&[
                endpoint.subsystem,
                endpoint.upstream,
                endpoint.endpoint,
                outcome.label(),
            ])
            .inc();
    }

    pub(crate) fn ejected(&self, endpoint: EndpointLabels<'_>) {
        self.ejections
            .with_label_values(&[endpoint.subsystem, endpoint.upstream, endpoint.endpoint])
            .inc();
    }

    pub(crate) fn health_check(&self, endpoint: EndpointLabels<'_>, ok: bool) {
        let result = if ok { "ok" } else { "fail" };
        self.health_checks
            .with_label_values(&[
                endpoint.subsystem,
                endpoint.upstream,
                endpoint.endpoint,
                result,
            ])
            .inc();
    }

    pub(crate) fn set_endpoint_state(
        &self,
        endpoint: EndpointLabels<'_>,
        healthy: bool,
        in_flight: i64,
    ) {
        let labels = [endpoint.subsystem, endpoint.upstream, endpoint.endpoint];
        self.endpoint_healthy
            .with_label_values(&labels)
            .set(i64::from(healthy));
        self.endpoint_in_flight
            .with_label_values(&labels)
            .set(in_flight);
    }

    pub(crate) fn retry(&self, subsystem: &str, upstream: &str, reason: RetryReason) {
        self.retries
            .with_label_values(&[subsystem, upstream, reason.label()])
            .inc();
    }

    pub(crate) fn budget_exhausted(&self, subsystem: &str, upstream: &str) {
        self.budget_exhausted
            .with_label_values(&[subsystem, upstream])
            .inc();
    }

    pub(crate) fn panic_pick(&self, subsystem: &str, upstream: &str) {
        self.panics.with_label_values(&[subsystem, upstream]).inc();
    }

    /// Deletes every series whose endpoint (or group, for group-level
    /// series) is not in `live`.
    pub(crate) fn retain(&self, live: &HashSet<(String, String, String)>) {
        let live_groups: HashSet<(&str, &str)> = live
            .iter()
            .map(|(subsystem, upstream, _)| (subsystem.as_str(), upstream.as_str()))
            .collect();
        let endpoint_live = |labels: &[String]| {
            live.contains(&(labels[0].clone(), labels[1].clone(), labels[2].clone()))
        };
        let group_live =
            |labels: &[String]| live_groups.contains(&(labels[0].as_str(), labels[1].as_str()));
        prune(
            &self.requests,
            &ENDPOINT_REQUEST_LABELS,
            endpoint_live,
            |v| {
                let _ = self.requests.remove_label_values(v);
            },
        );
        prune(
            &self.health_checks,
            &HEALTH_CHECK_LABELS,
            endpoint_live,
            |v| {
                let _ = self.health_checks.remove_label_values(v);
            },
        );
        prune(&self.ejections, &ENDPOINT_LABELS, endpoint_live, |v| {
            let _ = self.ejections.remove_label_values(v);
        });
        prune(
            &self.endpoint_healthy,
            &ENDPOINT_LABELS,
            endpoint_live,
            |v| {
                let _ = self.endpoint_healthy.remove_label_values(v);
            },
        );
        prune(
            &self.endpoint_in_flight,
            &ENDPOINT_LABELS,
            endpoint_live,
            |v| {
                let _ = self.endpoint_in_flight.remove_label_values(v);
            },
        );
        prune(&self.panics, &GROUP_LABELS, group_live, |v| {
            let _ = self.panics.remove_label_values(v);
        });
        prune(&self.budget_exhausted, &GROUP_LABELS, group_live, |v| {
            let _ = self.budget_exhausted.remove_label_values(v);
        });
        prune(&self.retries, &RETRY_LABELS, group_live, |v| {
            let _ = self.retries.remove_label_values(v);
        });
    }
}

/// Removes every series of `vec` whose labels `keep` rejects. A removal can
/// only fail if the series is already gone.
fn prune(
    vec: &impl Collector,
    names: &[&str],
    keep: impl Fn(&[String]) -> bool,
    remove: impl Fn(&[&str]),
) {
    for labels in label_sets(vec, names) {
        if !keep(&labels) {
            let values: Vec<&str> = labels.iter().map(String::as_str).collect();
            remove(&values);
        }
    }
}

/// The label values of every series currently in `vec`, ordered like `names`
/// (the gathered labels come back sorted by name, not in declaration order).
fn label_sets(vec: &impl Collector, names: &[&str]) -> Vec<Vec<String>> {
    vec.collect()
        .iter()
        .flat_map(|family| family.get_metric())
        .map(|metric| {
            names
                .iter()
                .map(|name| {
                    metric
                        .get_label()
                        .iter()
                        .find(|pair| pair.name() == *name)
                        .map(|pair| pair.value().to_string())
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Metrics;

    fn labels<'a>(endpoint: &'a str) -> EndpointLabels<'a> {
        EndpointLabels {
            subsystem: "shop",
            upstream: "backend",
            endpoint,
        }
    }

    fn live(endpoints: &[&str]) -> HashSet<(String, String, String)> {
        endpoints
            .iter()
            .map(|e| ("shop".to_string(), "backend".to_string(), e.to_string()))
            .collect()
    }

    #[test]
    fn requests_ejections_and_panics_are_counted_with_config_labels() {
        let metrics = Metrics::new();
        metrics.upstream_request(labels("a:1"), UpstreamOutcome::Success);
        metrics.upstream_request(labels("a:1"), UpstreamOutcome::Success);
        metrics.upstream_request(labels("a:1"), UpstreamOutcome::Connect);
        metrics.endpoint_ejected(labels("a:1"));
        metrics.upstream_panic("shop", "backend");
        let text = metrics.render();
        for expected in [
            r#"fastproxy_upstream_requests_total{endpoint="a:1",outcome="success",subsystem="shop",upstream="backend"} 2"#,
            r#"fastproxy_upstream_requests_total{endpoint="a:1",outcome="connect",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_endpoint_ejections_total{endpoint="a:1",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_upstream_panic_total{subsystem="shop",upstream="backend"} 1"#,
        ] {
            assert!(text.contains(expected), "missing {expected}\n{text}");
        }
    }

    #[test]
    fn retries_and_budget_refusals_are_counted_and_their_series_deleted_with_the_group() {
        let metrics = Metrics::new();
        metrics.upstream_retry("shop", "backend", RetryReason::Connect);
        metrics.upstream_retry("shop", "backend", RetryReason::BadStatus);
        metrics.retry_budget_exhausted("shop", "backend");
        metrics.upstream_retry("shop", "gone", RetryReason::Reset);
        metrics.retry_budget_exhausted("shop", "gone");
        let text = metrics.render();
        for expected in [
            r#"fastproxy_upstream_retries_total{reason="connect",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_upstream_retries_total{reason="bad_status",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_retry_budget_exhausted_total{subsystem="shop",upstream="backend"} 1"#,
        ] {
            assert!(text.contains(expected), "missing {expected}\n{text}");
        }
        for reason in [RetryReason::Timeout, RetryReason::Reset] {
            metrics.upstream_retry("shop", "backend", reason);
        }
        assert!(metrics.render().contains(r#"reason="timeout""#));

        metrics.retain_upstream_series(&live(&["a:1"]));
        let text = metrics.render();
        assert!(!text.contains(r#"upstream="gone""#), "{text}");
        assert!(text.contains(r#"upstream="backend""#));
    }

    #[test]
    fn health_checks_and_endpoint_gauges_are_reported_and_pruned() {
        let metrics = Metrics::new();
        metrics.health_check(labels("a:1"), true);
        metrics.health_check(labels("a:1"), false);
        metrics.health_check(labels("b:2"), true);
        metrics.set_endpoint_state(labels("a:1"), true, 3);
        metrics.set_endpoint_state(labels("b:2"), false, 0);
        let text = metrics.render();
        for expected in [
            r#"fastproxy_health_checks_total{endpoint="a:1",result="ok",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_health_checks_total{endpoint="a:1",result="fail",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_upstream_endpoint_healthy{endpoint="a:1",subsystem="shop",upstream="backend"} 1"#,
            r#"fastproxy_upstream_endpoint_healthy{endpoint="b:2",subsystem="shop",upstream="backend"} 0"#,
            r#"fastproxy_upstream_endpoint_in_flight{endpoint="a:1",subsystem="shop",upstream="backend"} 3"#,
        ] {
            assert!(text.contains(expected), "missing {expected}\n{text}");
        }
        metrics.retain_upstream_series(&live(&["a:1"]));
        let text = metrics.render();
        assert!(!text.contains(r#"endpoint="b:2""#), "{text}");
        assert!(text.contains(r#"endpoint="a:1""#));
    }

    #[test]
    fn every_outcome_has_its_own_label() {
        let metrics = Metrics::new();
        for outcome in [
            UpstreamOutcome::Success,
            UpstreamOutcome::BadStatus,
            UpstreamOutcome::Connect,
            UpstreamOutcome::Timeout,
            UpstreamOutcome::Other,
        ] {
            metrics.upstream_request(labels("a:1"), outcome);
        }
        let text = metrics.render();
        for outcome in ["success", "bad_status", "connect", "timeout", "other"] {
            assert!(
                text.contains(&format!("outcome=\"{outcome}\"")),
                "{outcome}"
            );
        }
    }

    #[test]
    fn series_of_removed_endpoints_and_groups_are_deleted() {
        let metrics = Metrics::new();
        for endpoint in ["a:1", "b:2"] {
            metrics.upstream_request(labels(endpoint), UpstreamOutcome::Success);
            metrics.endpoint_ejected(labels(endpoint));
        }
        metrics.upstream_panic("shop", "backend");
        metrics.upstream_panic("shop", "gone");

        metrics.retain_upstream_series(&live(&["a:1"]));
        let text = metrics.render();
        assert!(text.contains(r#"endpoint="a:1""#));
        assert!(!text.contains(r#"endpoint="b:2""#), "{text}");
        assert!(
            text.contains(r#"fastproxy_upstream_panic_total{subsystem="shop",upstream="backend"}"#)
        );
        assert!(!text.contains(r#"upstream="gone""#), "{text}");
    }
}
