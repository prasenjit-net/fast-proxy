//! TLS metrics. Certificate labels come from the certificate's admin-chosen
//! name (unique, at most 256), never from handshake data, so cardinality is
//! bounded by configuration. Handshakes are labelled by listener, result and
//! TLS version only.

use prometheus::{IntCounterVec, IntGaugeVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// How a TLS handshake ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeResult {
    Success,
    /// The client or the proxy aborted with a TLS alert or protocol error.
    Failed,
    /// The handshake did not finish within the timeout.
    Timeout,
    /// No certificate was selected and the proxy is set to reject unknown names.
    RejectedSni,
}

impl HandshakeResult {
    fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Timeout => "timeout",
            Self::RejectedSni => "rejected_sni",
        }
    }
}

/// Whether a certificate lookup reused a parsed entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheResult {
    /// An unchanged certificate reused its parsed entry.
    Hit,
    /// A new or changed certificate was parsed.
    Miss,
    /// A stored certificate could not be read.
    Unreadable,
}

impl CacheResult {
    fn label(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Unreadable => "unreadable",
        }
    }
}

/// The result of one cluster sync poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResult {
    /// Nothing had changed.
    Unchanged,
    /// A newer version was found and applied.
    Reloaded,
    /// Reading or applying the newer version failed.
    Failed,
}

impl SyncResult {
    fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Reloaded => "reloaded",
            Self::Failed => "failed",
        }
    }
}

/// One certificate's reading for the gauges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateReading {
    pub name: String,
    pub source: &'static str,
    /// Seconds until expiry; negative once expired.
    pub seconds_left: i64,
    pub unreadable: bool,
}

pub(crate) struct TlsMetrics {
    handshakes: IntCounterVec,
    cache: IntCounterVec,
    sync: IntCounterVec,
    expiry: IntGaugeVec,
    unreadable: IntGaugeVec,
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

impl TlsMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        Self {
            handshakes: counter(
                registry,
                "fastproxy_tls_handshakes_total",
                "TLS handshakes, by listener, result and negotiated version",
                &["listener", "result", "version"],
            ),
            cache: counter(
                registry,
                "fastproxy_cert_cache_total",
                "Certificate cache rebuild outcomes: reused, parsed, or unreadable",
                &["result"],
            ),
            sync: counter(
                registry,
                "fastproxy_config_sync_total",
                "Cluster sync polls, by result",
                &["result"],
            ),
            expiry: gauge(
                registry,
                "fastproxy_tls_cert_expiry_seconds",
                "Seconds until the certificate expires (negative once expired)",
                &["cert", "source"],
            ),
            unreadable: gauge(
                registry,
                "fastproxy_tls_cert_unreadable",
                "1 if the stored certificate cannot be read (for example a wrong master key)",
                &["cert"],
            ),
        }
    }

    pub(crate) fn handshake(&self, listener: &str, result: HandshakeResult, version: &str) {
        self.handshakes
            .with_label_values(&[listener, result.label(), version])
            .inc();
    }

    pub(crate) fn cache(&self, result: CacheResult, count: u64) {
        self.cache
            .with_label_values(&[result.label()])
            .inc_by(count);
    }

    pub(crate) fn sync(&self, result: SyncResult) {
        self.sync.with_label_values(&[result.label()]).inc();
    }

    /// Replaces the per-certificate series, which also deletes those of
    /// certificates that no longer exist.
    pub(crate) fn set_certificates(&self, readings: &[CertificateReading]) {
        self.expiry.reset();
        self.unreadable.reset();
        for reading in readings {
            if reading.unreadable {
                self.unreadable.with_label_values(&[&reading.name]).set(1);
            }
            self.expiry
                .with_label_values(&[&reading.name, reading.source])
                .set(reading.seconds_left);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{CertificateReading, HandshakeResult, Metrics, SyncResult};

    #[test]
    fn a_scrape_shows_each_tls_metric_after_known_events() {
        let metrics = Metrics::new();
        metrics.tls_handshake("https", HandshakeResult::Success, "TLSv1.3");
        metrics.tls_handshake("https", HandshakeResult::Success, "TLSv1.3");
        metrics.tls_handshake("https", HandshakeResult::Timeout, "-");
        metrics.cert_cache_rebuilt(3, 1, 2);
        metrics.config_sync(SyncResult::Reloaded);
        metrics.set_certificate_readings(&[
            CertificateReading {
                name: "shop".into(),
                source: "uploaded",
                seconds_left: 86_400,
                unreadable: false,
            },
            CertificateReading {
                name: "broken".into(),
                source: "self_signed",
                seconds_left: -5,
                unreadable: true,
            },
        ]);
        let text = metrics.render();
        for expected in [
            "fastproxy_tls_handshakes_total{listener=\"https\",result=\"success\",version=\"TLSv1.3\"} 2",
            "fastproxy_tls_handshakes_total{listener=\"https\",result=\"timeout\",version=\"-\"} 1",
            "fastproxy_cert_cache_total{result=\"hit\"} 3",
            "fastproxy_cert_cache_total{result=\"miss\"} 1",
            "fastproxy_cert_cache_total{result=\"unreadable\"} 2",
            "fastproxy_config_sync_total{result=\"reloaded\"} 1",
            "fastproxy_tls_cert_expiry_seconds{cert=\"shop\",source=\"uploaded\"} 86400",
            "fastproxy_tls_cert_expiry_seconds{cert=\"broken\",source=\"self_signed\"} -5",
            "fastproxy_tls_cert_unreadable{cert=\"broken\"} 1",
        ] {
            assert!(text.contains(expected), "missing {expected}\n{text}");
        }
        assert!(!text.contains("fastproxy_tls_cert_unreadable{cert=\"shop\"}"));
    }

    #[test]
    fn series_of_deleted_certificates_are_deleted() {
        let metrics = Metrics::new();
        let reading = |name: &str| CertificateReading {
            name: name.into(),
            source: "uploaded",
            seconds_left: 10,
            unreadable: true,
        };
        metrics.set_certificate_readings(&[reading("old"), reading("kept")]);
        metrics.set_certificate_readings(&[reading("kept")]);
        let text = metrics.render();
        assert!(!text.contains("cert=\"old\""), "{text}");
        assert!(text.contains("cert=\"kept\""));
    }
}
