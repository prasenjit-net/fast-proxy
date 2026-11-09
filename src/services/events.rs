use serde::Serialize;

use crate::services::metrics::MetricsSnapshot;

/// Server-push events delivered to every WebSocket client as JSON,
/// discriminated by a `type` field, e.g.:
///
/// ```json
/// { "type": "activity", "kind": "task", "message": "…", "timestampMs": 0 }
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
