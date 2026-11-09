//! Server-push event types sent over the WebSocket.

use serde::Serialize;

use crate::live_metrics::MetricsSnapshot;

/// Server-push events delivered to every WebSocket client as JSON,
/// discriminated by a `type` field, e.g.:
///
/// ```json
/// { "type": "activity", "kind": "config", "message": "…", "timestampMs": 0 }
/// ```
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    #[serde(rename_all = "camelCase")]
    Hello {
        message: String,
        timestamp_ms: i64,
    },

    Metrics {
        data: MetricsSnapshot,
    },

    #[serde(rename_all = "camelCase")]
    Activity {
        kind: String,
        message: String,
        timestamp_ms: i64,
    },
}
