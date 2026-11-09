//! Shared state for admin handlers.

use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

use fast_proxy_telemetry::AccessLog;
use tokio::sync::{broadcast, RwLock};

use crate::events::Event;
use crate::live_metrics::MetricsSnapshot;
use crate::ui_config::UiConfig;

/// Everything the admin plane's handlers share.
pub struct AdminState {
    pub ui: UiConfig,
    /// The binary's version; the admin crate cannot read it from its own manifest.
    pub version: &'static str,
    /// Fan-out channel feeding every connected WebSocket client.
    pub events: broadcast::Sender<Event>,
    pub latest_metrics: RwLock<Option<MetricsSnapshot>>,
    pub requests_total: AtomicU64,
    pub ws_clients: AtomicUsize,
    pub started: Instant,
    pub started_at_ms: i64,
    pub access_log: AccessLog,
}

pub type SharedState = Arc<AdminState>;

impl AdminState {
    pub fn new(ui: UiConfig, version: &'static str, access_log: AccessLog) -> Self {
        let (events, _) = broadcast::channel(64);
        Self {
            ui,
            version,
            events,
            latest_metrics: RwLock::new(None),
            requests_total: AtomicU64::new(0),
            ws_clients: AtomicUsize::new(0),
            started: Instant::now(),
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            access_log,
        }
    }

    pub fn broadcast(&self, event: Event) {
        // send() only fails when no client is subscribed — not an error.
        let _ = self.events.send(event);
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
