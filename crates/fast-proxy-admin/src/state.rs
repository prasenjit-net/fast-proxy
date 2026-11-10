//! Shared state for admin handlers.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;

use fast_proxy_config_service::ConfigService;
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use tokio::sync::{broadcast, RwLock};

use crate::events::Event;
use crate::runtime_info::RuntimeInfo;
use crate::security::AdminSecurity;
use crate::traffic::TrafficSnapshot;
use crate::ui_config::UiConfig;

/// Everything the admin plane's handlers share.
pub struct AdminState {
    pub ui: UiConfig,
    /// The only writer of configuration; handlers never touch storage directly.
    pub config: Arc<ConfigService>,
    pub runtime: RuntimeInfo,
    /// The binary's version; the admin crate cannot read it from its own manifest.
    pub version: &'static str,
    /// Fan-out channel feeding every connected WebSocket client.
    pub events: broadcast::Sender<Event>,
    pub latest_traffic: RwLock<Option<TrafficSnapshot>>,
    pub ws_clients: AtomicUsize,
    pub started: Instant,
    pub started_at_ms: i64,
    pub access_log: Arc<AccessLog>,
    /// The proxy's Prometheus metrics, rendered at `GET /metrics`.
    pub metrics: Arc<Metrics>,
    /// Endpoint health and ejection changes, forwarded to the UI.
    pub endpoint_events: EndpointEvents,
    /// Authentication, the `Host` allow-list, the throttle and WebSocket tickets.
    pub security: AdminSecurity,
}

pub type SharedState = Arc<AdminState>;

impl AdminState {
    // Each argument is a distinct collaborator the admin plane is wired from.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ui: UiConfig,
        version: &'static str,
        access_log: Arc<AccessLog>,
        config: Arc<ConfigService>,
        runtime: RuntimeInfo,
        metrics: Arc<Metrics>,
        endpoint_events: EndpointEvents,
        security: AdminSecurity,
    ) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            ui,
            config,
            runtime,
            version,
            events,
            latest_traffic: RwLock::new(None),
            ws_clients: AtomicUsize::new(0),
            started: Instant::now(),
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            access_log,
            metrics,
            endpoint_events,
            security,
        }
    }

    pub fn broadcast(&self, event: Event) {
        // send() only fails when no client is subscribed — not an error.
        let _ = self.events.send(event);
    }

    /// Tells every client a certificate changed, with a matching activity entry.
    /// Only the certificate's name is sent, never key material.
    pub fn certificate_changed(&self, change: &'static str, certificate: &str) {
        let timestamp_ms = chrono::Utc::now().timestamp_millis();
        self.broadcast(Event::CertState {
            change,
            certificate: certificate.to_string(),
            timestamp_ms,
        });
        self.activity(
            "certificate",
            format!("certificate {change}: {certificate}"),
        );
    }

    /// Tells every client a token was created or revoked, with a matching
    /// activity entry. Only the token's name is sent.
    pub fn token_changed(&self, change: &'static str, token: &str) {
        self.broadcast(Event::TokenChanged {
            change,
            token: token.to_string(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        });
        self.activity("token", format!("token {change}: {token}"));
    }

    /// Push a human-readable entry to every client's activity feed.
    pub fn activity(&self, kind: &str, message: impl Into<String>) {
        self.broadcast(Event::Activity {
            kind: kind.to_string(),
            message: message.into(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        });
    }
}
