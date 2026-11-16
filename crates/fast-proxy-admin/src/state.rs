//! Shared state for admin handlers.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Instant;

use fast_proxy_config_service::ConfigService;
use fast_proxy_sample_service::SampleReader;
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
    /// Reads the sampled requests of every instance sharing the database.
    pub samples: Arc<SampleReader>,
    /// ACME, when it is on for the process.
    pub acme: Option<AcmeAdmin>,
}

/// What the admin plane knows about ACME on this process: the settings it
/// shows, and the runner it can ask for a renewal.
pub struct AcmeAdmin {
    pub directory: String,
    /// The directory is Let's Encrypt staging (untrusted certificates).
    pub staging: bool,
    pub allowlist: fast_proxy_model::DomainAllowlist,
    pub control: fast_proxy_acme_service::AcmeControl,
}

pub type SharedState = Arc<AdminState>;

/// What changed about one maintenance scope, for [`AdminState::maintenance_changed`].
pub struct MaintenanceChange<'a> {
    pub scope: &'static str,
    pub subsystem: Option<&'a str>,
    pub route: Option<&'a str>,
    pub active: bool,
    pub expires_at_ms: Option<i64>,
}

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
        samples: Arc<SampleReader>,
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
            samples,
            acme: None,
        }
    }

    /// Turns the ACME endpoints on, for a process with ACME on.
    pub fn with_acme(mut self, acme: AcmeAdmin) -> Self {
        self.acme = Some(acme);
        self
    }

    /// Tells every client sampling changed: `settings`, `override` or
    /// `cleared` (the stored samples were deleted).
    pub fn sampling_changed(&self, change: &'static str) {
        self.broadcast(Event::SamplingChanged {
            change,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        });
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

    /// Tells every client maintenance was turned on or off for a scope, with a
    /// matching activity entry.
    pub fn maintenance_changed(&self, change: MaintenanceChange<'_>) {
        let verb = if change.active { "on" } else { "off" };
        let target = match (change.subsystem, change.route) {
            (Some(subsystem), Some(route)) => format!("{subsystem} {route}"),
            (Some(subsystem), None) => subsystem.to_string(),
            _ => "the whole proxy".to_string(),
        };
        self.broadcast(Event::MaintenanceChanged {
            scope: change.scope,
            subsystem: change.subsystem.map(str::to_string),
            route: change.route.map(str::to_string),
            active: change.active,
            expires_at_ms: change.expires_at_ms,
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        });
        self.activity("maintenance", format!("maintenance {verb}: {target}"));
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
