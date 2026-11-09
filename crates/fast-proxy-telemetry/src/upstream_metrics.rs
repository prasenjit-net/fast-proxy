//! Per-endpoint metrics. Label values come only from config names (subsystem
//! and upstream names, endpoint `host:port`), so cardinality is bounded by
//! the configuration; series of removed endpoints are deleted.

use std::collections::HashSet;

use prometheus::{IntCounterVec, Opts, Registry};

const ENDPOINT_REQUEST_LABELS: [&str; 4] = ["subsystem", "upstream", "endpoint", "outcome"];
const ENDPOINT_LABELS: [&str; 3] = ["subsystem", "upstream", "endpoint"];
const GROUP_LABELS: [&str; 2] = ["subsystem", "upstream"];
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
}

fn counter(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let counter = IntCounterVec::new(Opts::new(name, help), labels).expect(REGISTRATION);
    registry
        .register(Box::new(counter.clone()))
        .expect(REGISTRATION);
    counter
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

    pub(crate) fn panic_pick(&self, subsystem: &str, upstream: &str) {
        self.panics.with_label_values(&[subsystem, upstream]).inc();
    }

    /// Deletes every series whose endpoint (or group, for panic picks) is
    /// not in `live`.
    pub(crate) fn retain(&self, live: &HashSet<(String, String, String)>) {
        let live_groups: HashSet<(&str, &str)> = live
            .iter()
            .map(|(subsystem, upstream, _)| (subsystem.as_str(), upstream.as_str()))
            .collect();
        let per_endpoint = [
            (&self.requests, &ENDPOINT_REQUEST_LABELS[..]),
            (&self.ejections, &ENDPOINT_LABELS[..]),
        ];
        for (vec, names) in per_endpoint {
            for labels in label_sets(vec, names) {
                let key = (labels[0].clone(), labels[1].clone(), labels[2].clone());
                if !live.contains(&key) {
                    let values: Vec<&str> = labels.iter().map(String::as_str).collect();
                    // The series was just read, so removal can only fail if a
                    // concurrent retain already took it.
                    let _ = vec.remove_label_values(&values);
                }
            }
        }
        for labels in label_sets(&self.panics, &GROUP_LABELS) {
            if !live_groups.contains(&(labels[0].as_str(), labels[1].as_str())) {
                let values: Vec<&str> = labels.iter().map(String::as_str).collect();
                let _ = self.panics.remove_label_values(&values);
            }
        }
    }
}

/// The label values of every series currently in `vec`, ordered like `names`
/// (the gathered labels come back sorted by name, not in declaration order).
fn label_sets(vec: &IntCounterVec, names: &[&str]) -> Vec<Vec<String>> {
    use prometheus::core::Collector;
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
