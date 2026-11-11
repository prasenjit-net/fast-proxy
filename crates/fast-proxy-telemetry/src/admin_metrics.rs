//! Admin plane metrics: how its requests ended and how many audit entries were
//! written. Labels come from two fixed sets, never from a request, a token or a
//! path, so cardinality is bounded.

use prometheus::{IntCounterVec, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// How an admin request ended, as far as the guard is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminResult {
    /// An authenticated caller was served (or the handler itself refused it).
    Ok,
    /// No token, a wrong, expired or revoked one, or a bad ticket (401).
    Unauthenticated,
    /// A valid token whose role may not do this (403).
    Forbidden,
    /// The source address is throttled after repeated failures (429).
    Throttled,
    /// A `Host` header that is not an allowed admin name (421).
    HostRefused,
}

impl AdminResult {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::Throttled => "throttled",
            Self::HostRefused => "host_refused",
        }
    }
}

/// The outcome recorded in an audit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditResult {
    Ok,
    Denied,
    Failed,
}

impl AuditResult {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }
}

pub(crate) struct AdminMetrics {
    requests: IntCounterVec,
    audit_entries: IntCounterVec,
}

impl AdminMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        Self {
            requests: counter(
                registry,
                "fastproxy_admin_requests_total",
                "Admin requests by how the guard ended them: ok, unauthenticated, forbidden, \
                 throttled or host_refused",
                "result",
            ),
            audit_entries: counter(
                registry,
                "fastproxy_audit_entries_total",
                "Audit entries written by this instance, by outcome: ok, denied or failed",
                "outcome",
            ),
        }
    }

    pub(crate) fn request(&self, result: AdminResult) {
        self.requests.with_label_values(&[result.label()]).inc();
    }

    pub(crate) fn audit_entry(&self, outcome: AuditResult) {
        self.audit_entries
            .with_label_values(&[outcome.label()])
            .inc();
    }
}

fn counter(registry: &Registry, name: &str, help: &str, label: &str) -> IntCounterVec {
    let counter = IntCounterVec::new(Opts::new(name, help), &[label]).expect(REGISTRATION);
    registry
        .register(Box::new(counter.clone()))
        .expect(REGISTRATION);
    counter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_result_and_outcome_has_its_own_series() {
        let registry = Registry::new();
        let metrics = AdminMetrics::new(&registry);
        for result in [
            AdminResult::Ok,
            AdminResult::Unauthenticated,
            AdminResult::Forbidden,
            AdminResult::Throttled,
            AdminResult::HostRefused,
        ] {
            metrics.request(result);
        }
        metrics.request(AdminResult::Ok);
        for outcome in [AuditResult::Ok, AuditResult::Denied, AuditResult::Failed] {
            metrics.audit_entry(outcome);
        }
        let text = prometheus::TextEncoder::new()
            .encode_to_string(&registry.gather())
            .unwrap();
        assert!(
            text.contains(r#"fastproxy_admin_requests_total{result="ok"} 2"#),
            "{text}"
        );
        for result in ["unauthenticated", "forbidden", "throttled", "host_refused"] {
            assert!(
                text.contains(&format!(
                    r#"fastproxy_admin_requests_total{{result="{result}"}} 1"#
                )),
                "{result}: {text}"
            );
        }
        for outcome in ["ok", "denied", "failed"] {
            assert!(
                text.contains(&format!(
                    r#"fastproxy_audit_entries_total{{outcome="{outcome}"}} 1"#
                )),
                "{outcome}: {text}"
            );
        }
    }
}
