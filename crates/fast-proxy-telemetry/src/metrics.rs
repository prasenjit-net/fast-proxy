use std::collections::HashMap;
use std::time::Duration;

use crate::maintenance_metrics::{MaintenanceMetrics, MaintenanceReading, RESPONSES_METRIC};
use crate::tls_metrics::{
    CacheResult, CertificateReading, HandshakeResult, SyncResult, TlsMetrics,
};
use crate::upstream_metrics::{EndpointLabels, RetryReason, UpstreamMetrics, UpstreamOutcome};
use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};

/// Label value for traffic that matched no subsystem.
pub const UNMATCHED_SUBSYSTEM: &str = "_unmatched";
/// Label value for traffic that matched no route.
pub const NO_ROUTE: &str = "_none";

/// Why a request to an upstream failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamErrorKind {
    /// The connection could not be established (refused, unreachable).
    Connect,
    /// The upstream did not answer in time.
    Timeout,
    /// Anything else that went wrong talking to the upstream.
    Other,
}

impl UpstreamErrorKind {
    fn label(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Other => "other",
        }
    }
}

/// Whether a configuration change was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadOutcome {
    Applied,
    Rejected,
}

impl ReloadOutcome {
    fn label(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::Rejected => "rejected",
        }
    }
}

/// Request and error counts since startup, for one subsystem or in total.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub requests: u64,
    /// Responses with a 5xx status.
    pub errors: u64,
}

/// A point-in-time reading of the traffic counters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrafficTotals {
    pub per_subsystem: HashMap<String, Counts>,
    pub active_connections: i64,
    pub active_requests: i64,
}

impl TrafficTotals {
    /// Totals across every subsystem.
    pub fn overall(&self) -> Counts {
        self.per_subsystem
            .values()
            .fold(Counts::default(), |sum, counts| Counts {
                requests: sum.requests + counts.requests,
                errors: sum.errors + counts.errors,
            })
    }
}

/// The proxy's Prometheus metrics. Label values only ever come from config
/// names (subsystem names, route prefixes), never from raw hosts or paths,
/// so cardinality is bounded by the configuration.
pub struct Metrics {
    registry: Registry,
    requests: IntCounterVec,
    duration: HistogramVec,
    upstream_errors: IntCounterVec,
    active_connections: IntGauge,
    active_requests: IntGauge,
    config_version: IntGauge,
    config_reloads: IntCounterVec,
    cancelled: IntCounterVec,
    upstream: UpstreamMetrics,
    tls: TlsMetrics,
    maintenance: MaintenanceMetrics,
}

/// Metric names and label sets are fixed at compile time and registered
/// once on a fresh registry, so registration cannot fail.
const REGISTRATION: &str = "static metric definitions register on a fresh registry";

fn counter_vec(registry: &Registry, name: &str, help: &str, labels: &[&str]) -> IntCounterVec {
    let counter = IntCounterVec::new(Opts::new(name, help), labels).expect(REGISTRATION);
    registry
        .register(Box::new(counter.clone()))
        .expect(REGISTRATION);
    counter
}

fn gauge(registry: &Registry, name: &str, help: &str) -> IntGauge {
    let gauge = IntGauge::new(name, help).expect(REGISTRATION);
    registry
        .register(Box::new(gauge.clone()))
        .expect(REGISTRATION);
    gauge
}

impl Metrics {
    pub fn new() -> Self {
        let registry = Registry::new();
        let duration = HistogramVec::new(
            HistogramOpts::new(
                "fastproxy_request_duration_seconds",
                "Time from receiving a request to finishing its response",
            ),
            &["subsystem", "route"],
        )
        .expect(REGISTRATION);
        registry
            .register(Box::new(duration.clone()))
            .expect(REGISTRATION);
        let upstream = UpstreamMetrics::new(&registry);
        let tls = TlsMetrics::new(&registry);
        let maintenance = MaintenanceMetrics::new(&registry);
        Self {
            upstream,
            tls,
            maintenance,
            cancelled: counter_vec(
                &registry,
                "fastproxy_requests_cancelled_total",
                "Requests whose client left before the response head, by subsystem and route",
                &["subsystem", "route"],
            ),
            requests: counter_vec(
                &registry,
                "fastproxy_requests_total",
                "Requests handled, by subsystem, route, and response status class",
                &["subsystem", "route", "status_class"],
            ),
            upstream_errors: counter_vec(
                &registry,
                "fastproxy_upstream_errors_total",
                "Failed upstream requests, by subsystem and kind",
                &["subsystem", "kind"],
            ),
            config_reloads: counter_vec(
                &registry,
                "fastproxy_config_reloads_total",
                "Configuration changes, by outcome",
                &["outcome"],
            ),
            active_connections: gauge(
                &registry,
                "fastproxy_active_connections",
                "Open client connections",
            ),
            active_requests: gauge(
                &registry,
                "fastproxy_active_requests",
                "Requests being handled right now",
            ),
            config_version: gauge(
                &registry,
                "fastproxy_config_version",
                "The live configuration version",
            ),
            duration,
            registry,
        }
    }

    /// Records one finished request. `route` is the matched route's prefix,
    /// or [`NO_ROUTE`]; `subsystem` is its name, or [`UNMATCHED_SUBSYSTEM`].
    pub fn observe_request(&self, subsystem: &str, route: &str, status: u16, elapsed: Duration) {
        self.requests
            .with_label_values(&[subsystem, route, status_class(status)])
            .inc();
        self.duration
            .with_label_values(&[subsystem, route])
            .observe(elapsed.as_secs_f64());
    }

    /// Counts a request answered with the maintenance response by `scope`.
    /// `subsystem` and `route` are the ones the request matched (`_none` for a
    /// route not yet matched).
    pub fn maintenance_response(&self, scope: &str, subsystem: &str, route: &str) {
        self.maintenance.response(scope, subsystem, route);
    }

    /// Counts a request that passed through a maintenance scope by a bypass rule.
    pub fn maintenance_bypass(&self, scope: &str, subsystem: &str) {
        self.maintenance.bypass(scope, subsystem);
    }

    /// Sets which maintenance scopes are on; any not listed read as off.
    pub fn set_maintenance_active(&self, readings: &[MaintenanceReading]) {
        self.maintenance.set_active(readings);
    }

    /// Counts a request whose client closed the connection before the proxy
    /// had a response.
    pub fn request_cancelled(&self, subsystem: &str, route: &str) {
        self.cancelled.with_label_values(&[subsystem, route]).inc();
    }

    pub fn upstream_error(&self, subsystem: &str, kind: UpstreamErrorKind) {
        self.upstream_errors
            .with_label_values(&[subsystem, kind.label()])
            .inc();
    }

    /// Counts one request sent to an endpoint, by how it turned out.
    pub fn upstream_request(&self, endpoint: EndpointLabels<'_>, outcome: UpstreamOutcome) {
        self.upstream.request(endpoint, outcome);
    }

    /// Counts one active health probe of an endpoint.
    pub fn health_check(&self, endpoint: EndpointLabels<'_>, ok: bool) {
        self.upstream.health_check(endpoint, ok);
    }

    /// Publishes an endpoint's routability and in-flight count as gauges.
    pub fn set_endpoint_state(&self, endpoint: EndpointLabels<'_>, healthy: bool, in_flight: i64) {
        self.upstream
            .set_endpoint_state(endpoint, healthy, in_flight);
    }

    /// Counts one ejection of an endpoint.
    pub fn endpoint_ejected(&self, endpoint: EndpointLabels<'_>) {
        self.upstream.ejected(endpoint);
    }

    /// Counts one retry of a request on another endpoint.
    pub fn upstream_retry(&self, subsystem: &str, upstream: &str, reason: RetryReason) {
        self.upstream.retry(subsystem, upstream, reason);
    }

    /// Counts a retry refused because the group's budget is spent.
    pub fn retry_budget_exhausted(&self, subsystem: &str, upstream: &str) {
        self.upstream.budget_exhausted(subsystem, upstream);
    }

    /// Counts a request served in panic mode by an upstream group.
    pub fn upstream_panic(&self, subsystem: &str, upstream: &str) {
        self.upstream.panic_pick(subsystem, upstream);
    }

    /// Deletes the per-endpoint series of endpoints no longer configured.
    /// `live` holds `(subsystem, upstream, endpoint)` for every configured one.
    pub fn retain_upstream_series(
        &self,
        live: &std::collections::HashSet<(String, String, String)>,
    ) {
        self.upstream.retain(live);
    }

    /// Counts one TLS handshake on `listener`; `version` is the negotiated
    /// protocol (`TLSv1.3`) or `-` when none was negotiated.
    pub fn tls_handshake(&self, listener: &str, result: HandshakeResult, version: &str) {
        self.tls.handshake(listener, result, version);
    }

    /// Adds the outcome counts of one certificate cache rebuild.
    pub fn cert_cache_rebuilt(&self, hits: u64, misses: u64, unreadable: u64) {
        self.tls.cache(CacheResult::Hit, hits);
        self.tls.cache(CacheResult::Miss, misses);
        self.tls.cache(CacheResult::Unreadable, unreadable);
    }

    /// Counts one cluster sync poll.
    pub fn config_sync(&self, result: SyncResult) {
        self.tls.sync(result);
    }

    /// Publishes the per-certificate expiry and unreadable gauges, replacing
    /// every earlier series so deleted certificates disappear.
    pub fn set_certificate_readings(&self, readings: &[CertificateReading]) {
        self.tls.set_certificates(readings);
    }

    /// Publishes which upstream groups use TLS and whether they verify
    /// certificates, replacing the earlier series.
    pub fn set_upstream_tls_groups(&self, groups: &[(String, String, bool)]) {
        self.tls.set_upstream_tls(groups);
    }

    /// Counts a request as active until the returned guard drops.
    pub fn request_started(&self) -> ActiveRequest {
        self.active_requests.inc();
        ActiveRequest {
            gauge: self.active_requests.clone(),
        }
    }

    /// Counts one more open client connection.
    pub fn connection_opened(&self) {
        self.active_connections.inc();
    }

    /// Counts one fewer open client connection.
    pub fn connection_closed(&self) {
        self.active_connections.dec();
    }

    pub fn set_config_version(&self, version: u64) {
        self.config_version
            .set(i64::try_from(version).unwrap_or(i64::MAX));
    }

    pub fn config_reload(&self, outcome: ReloadOutcome) {
        self.config_reloads
            .with_label_values(&[outcome.label()])
            .inc();
    }

    /// The Prometheus text exposition of every metric.
    pub fn render(&self) -> String {
        let mut buffer = Vec::new();
        // Encoding into a Vec cannot fail; an empty body is the safe fallback.
        if TextEncoder::new()
            .encode(&self.registry.gather(), &mut buffer)
            .is_err()
        {
            buffer.clear();
        }
        String::from_utf8(buffer).unwrap_or_default()
    }

    /// Reads the cumulative request and error counts per subsystem, plus the
    /// current gauges, for the live traffic sampler.
    pub fn traffic_totals(&self) -> TrafficTotals {
        let mut totals = TrafficTotals {
            active_connections: self.active_connections.get(),
            active_requests: self.active_requests.get(),
            ..TrafficTotals::default()
        };
        let mut maintenance_answers: HashMap<String, u64> = HashMap::new();
        for family in self.registry.gather() {
            if family.name() == RESPONSES_METRIC {
                for metric in family.get_metric() {
                    let subsystem = metric
                        .get_label()
                        .iter()
                        .find(|pair| pair.name() == "subsystem")
                        .map(|pair| pair.value().to_string())
                        .unwrap_or_default();
                    *maintenance_answers.entry(subsystem).or_default() +=
                        metric.get_counter().value() as u64;
                }
                continue;
            }
            if family.name() != "fastproxy_requests_total" {
                continue;
            }
            for metric in family.get_metric() {
                let label = |name: &str| {
                    metric
                        .get_label()
                        .iter()
                        .find(|pair| pair.name() == name)
                        .map(|pair| pair.value().to_string())
                        .unwrap_or_default()
                };
                let count = metric.get_counter().value() as u64;
                let entry = totals.per_subsystem.entry(label("subsystem")).or_default();
                entry.requests += count;
                if label("status_class") == "5xx" {
                    entry.errors += count;
                }
            }
        }
        // A maintenance answer is a 503 on purpose, so it is not an error.
        for (subsystem, answers) in maintenance_answers {
            if let Some(counts) = totals.per_subsystem.get_mut(&subsystem) {
                counts.errors = counts.errors.saturating_sub(answers);
            }
        }
        totals
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps one request counted as active while alive.
pub struct ActiveRequest {
    gauge: IntGauge,
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.gauge.dec();
    }
}

fn status_class(status: u16) -> &'static str {
    match status / 100 {
        1 => "1xx",
        2 => "2xx",
        3 => "3xx",
        4 => "4xx",
        5 => "5xx",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value of the exposition line that starts with `prefix`.
    fn value_of(text: &str, prefix: &str) -> Option<f64> {
        text.lines()
            .find(|line| line.starts_with(prefix))
            .and_then(|line| line.rsplit(' ').next())
            .and_then(|value| value.parse().ok())
    }

    #[test]
    fn requests_are_counted_by_subsystem_route_and_status_class() {
        let metrics = Metrics::new();
        metrics.observe_request("shop", "/api", 200, Duration::from_millis(5));
        metrics.observe_request("shop", "/api", 204, Duration::from_millis(5));
        metrics.observe_request("shop", "/api", 502, Duration::from_millis(5));
        metrics.observe_request("blog", "/", 404, Duration::from_millis(5));
        let text = metrics.render();
        let line = |labels: &str| value_of(&text, &format!("fastproxy_requests_total{{{labels}}}"));
        assert_eq!(
            line(r#"route="/api",status_class="2xx",subsystem="shop""#),
            Some(2.0)
        );
        assert_eq!(
            line(r#"route="/api",status_class="5xx",subsystem="shop""#),
            Some(1.0)
        );
        assert_eq!(
            line(r#"route="/",status_class="4xx",subsystem="blog""#),
            Some(1.0)
        );
    }

    #[test]
    fn cancelled_requests_are_counted_by_subsystem_and_route() {
        let metrics = Metrics::new();
        metrics.request_cancelled("shop", "/api");
        metrics.request_cancelled("shop", "/api");
        let text = metrics.render();
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_requests_cancelled_total{route="/api",subsystem="shop"}"#
            ),
            Some(2.0)
        );
    }

    #[test]
    fn durations_form_a_histogram_per_subsystem_and_route() {
        let metrics = Metrics::new();
        metrics.observe_request("shop", "/", 200, Duration::from_millis(30));
        let text = metrics.render();
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_request_duration_seconds_count{route="/",subsystem="shop"}"#
            ),
            Some(1.0)
        );
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_request_duration_seconds_bucket{route="/",subsystem="shop",le="0.05"}"#
            ),
            Some(1.0)
        );
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_request_duration_seconds_bucket{route="/",subsystem="shop",le="0.025"}"#
            ),
            Some(0.0)
        );
    }

    #[test]
    fn upstream_errors_are_counted_by_kind() {
        let metrics = Metrics::new();
        metrics.upstream_error("shop", UpstreamErrorKind::Connect);
        metrics.upstream_error("shop", UpstreamErrorKind::Timeout);
        metrics.upstream_error("shop", UpstreamErrorKind::Timeout);
        metrics.upstream_error("shop", UpstreamErrorKind::Other);
        let text = metrics.render();
        for (kind, expected) in [("connect", 1.0), ("timeout", 2.0), ("other", 1.0)] {
            let prefix =
                format!(r#"fastproxy_upstream_errors_total{{kind="{kind}",subsystem="shop"}}"#);
            assert_eq!(value_of(&text, &prefix), Some(expected), "{kind}");
        }
    }

    #[test]
    fn active_requests_follow_their_guards() {
        let metrics = Metrics::new();
        let first = metrics.request_started();
        let second = metrics.request_started();
        assert_eq!(
            value_of(&metrics.render(), "fastproxy_active_requests"),
            Some(2.0)
        );
        drop(first);
        drop(second);
        assert_eq!(
            value_of(&metrics.render(), "fastproxy_active_requests"),
            Some(0.0)
        );
    }

    #[test]
    fn connections_are_counted_as_they_open_and_close() {
        let metrics = Metrics::new();
        metrics.connection_opened();
        metrics.connection_opened();
        metrics.connection_closed();
        assert_eq!(
            value_of(&metrics.render(), "fastproxy_active_connections"),
            Some(1.0)
        );
    }

    #[test]
    fn config_version_and_reloads_are_reported() {
        let metrics = Metrics::new();
        metrics.set_config_version(7);
        metrics.config_reload(ReloadOutcome::Applied);
        metrics.config_reload(ReloadOutcome::Applied);
        metrics.config_reload(ReloadOutcome::Rejected);
        let text = metrics.render();
        assert_eq!(value_of(&text, "fastproxy_config_version"), Some(7.0));
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_config_reloads_total{outcome="applied"}"#
            ),
            Some(2.0)
        );
        assert_eq!(
            value_of(
                &text,
                r#"fastproxy_config_reloads_total{outcome="rejected"}"#
            ),
            Some(1.0)
        );
    }

    #[test]
    fn the_exposition_names_every_metric_family() {
        let metrics = Metrics::new();
        metrics.observe_request(UNMATCHED_SUBSYSTEM, NO_ROUTE, 404, Duration::from_millis(1));
        metrics.upstream_error("shop", UpstreamErrorKind::Other);
        metrics.config_reload(ReloadOutcome::Applied);
        let text = metrics.render();
        for name in [
            "fastproxy_requests_total",
            "fastproxy_request_duration_seconds",
            "fastproxy_upstream_errors_total",
            "fastproxy_active_connections",
            "fastproxy_active_requests",
            "fastproxy_config_version",
            "fastproxy_config_reloads_total",
        ] {
            assert!(text.contains(&format!("# TYPE {name} ")), "missing {name}");
        }
        assert!(text.contains(r#"subsystem="_unmatched""#));
        assert!(text.contains(r#"route="_none""#));
    }

    #[test]
    fn traffic_totals_sum_requests_and_5xx_errors_per_subsystem() {
        let metrics = Metrics::new();
        metrics.observe_request("shop", "/", 200, Duration::from_millis(1));
        metrics.observe_request("shop", "/api", 200, Duration::from_millis(1));
        metrics.observe_request("shop", "/api", 503, Duration::from_millis(1));
        metrics.observe_request("blog", "/", 404, Duration::from_millis(1));
        let _active = metrics.request_started();
        metrics.connection_opened();
        metrics.connection_opened();
        metrics.connection_opened();

        let totals = metrics.traffic_totals();
        assert_eq!(
            totals.per_subsystem["shop"],
            Counts {
                requests: 3,
                errors: 1
            }
        );
        assert_eq!(
            totals.per_subsystem["blog"],
            Counts {
                requests: 1,
                errors: 0
            }
        );
        assert_eq!(
            totals.overall(),
            Counts {
                requests: 4,
                errors: 1
            }
        );
        assert_eq!(totals.active_requests, 1);
        assert_eq!(totals.active_connections, 3);
    }

    #[test]
    fn status_classes_cover_every_range() {
        let classes: Vec<&str> = [101, 200, 301, 404, 503, 99, 700]
            .map(status_class)
            .to_vec();
        assert_eq!(
            classes,
            ["1xx", "2xx", "3xx", "4xx", "5xx", "other", "other"]
        );
    }

    #[test]
    fn maintenance_answers_are_counted_and_do_not_count_as_errors() {
        let metrics = Metrics::new();
        let elapsed = Duration::from_millis(1);
        for _ in 0..3 {
            metrics.observe_request("shop", "_none", 503, elapsed);
            metrics.maintenance_response("global", "shop", "_none");
        }
        metrics.observe_request("shop", "/", 502, elapsed);
        metrics.observe_request("shop", "/", 200, elapsed);
        let totals = metrics.traffic_totals();
        let shop = totals.per_subsystem["shop"];
        assert_eq!(shop.requests, 5);
        assert_eq!(shop.errors, 1, "only the 502 is an error");
    }

    #[test]
    fn a_burst_of_maintenance_answers_leaves_the_error_rate_at_zero() {
        let metrics = Metrics::new();
        for _ in 0..50 {
            metrics.observe_request("shop", "_none", 503, Duration::from_millis(1));
            metrics.maintenance_response("subsystem", "shop", "_none");
        }
        assert_eq!(metrics.traffic_totals().overall().errors, 0);
    }

    #[test]
    fn the_maintenance_series_render_with_names_from_configuration_only() {
        let metrics = Metrics::new();
        metrics.maintenance_response("route", "shop", "/api");
        metrics.maintenance_bypass("global", "shop");
        metrics.set_maintenance_active(&[MaintenanceReading {
            scope: "route",
            subsystem: "shop".into(),
            route: "/api".into(),
        }]);
        let text = metrics.render();
        assert!(text.contains(
            r#"fastproxy_maintenance_active{route="/api",scope="route",subsystem="shop"} 1"#
        ));
        assert!(text.contains(
            r#"fastproxy_maintenance_responses_total{route="/api",scope="route",subsystem="shop"} 1"#
        ));
        assert!(text
            .contains(r#"fastproxy_maintenance_bypass_total{scope="global",subsystem="shop"} 1"#));
    }

    #[test]
    fn a_scope_that_ended_disappears_from_the_active_gauge() {
        let metrics = Metrics::new();
        let reading = MaintenanceReading {
            scope: "global",
            subsystem: "_none".into(),
            route: "_none".into(),
        };
        metrics.set_maintenance_active(std::slice::from_ref(&reading));
        assert!(metrics.render().contains("fastproxy_maintenance_active{"));
        metrics.set_maintenance_active(&[]);
        assert!(!metrics.render().contains("fastproxy_maintenance_active{"));
    }
}
