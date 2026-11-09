use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, RwLock};

use crate::access_log::AccessLog;
use crate::config::AppConfig;
use crate::services::events::Event;
use crate::services::metrics::MetricsSnapshot;
use crate::services::tasks::TaskStore;

pub struct AppState {
    pub config: AppConfig,
    /// Fan-out channel feeding every connected WebSocket client.
    pub events: broadcast::Sender<Event>,
    pub tasks: TaskStore,
    pub latest_metrics: RwLock<Option<MetricsSnapshot>>,
    pub requests_total: AtomicU64,
    pub ws_clients: AtomicUsize,
    pub started: Instant,
    pub started_at_ms: i64,
    pub access_log: AccessLog,
}

pub type SharedState = Arc<AppState>;

impl AppState {
    pub async fn new(config: AppConfig) -> Self {
        let (events, _) = broadcast::channel(64);
        let access_log = AccessLog::open(config.logging.access_log.as_deref()).await;
        Self {
            events,
            tasks: TaskStore::with_examples(),
            latest_metrics: RwLock::new(None),
            requests_total: AtomicU64::new(0),
            ws_clients: AtomicUsize::new(0),
            started: Instant::now(),
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            access_log,
            config,
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
