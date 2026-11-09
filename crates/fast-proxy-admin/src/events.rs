//! Server-push event types sent over the WebSocket.

use serde::Serialize;

use crate::traffic::TrafficSnapshot;

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
    Hello { message: String, timestamp_ms: i64 },

    /// Live traffic rates, pushed every sampling interval.
    Traffic { data: TrafficSnapshot },

    /// A configuration change went live.
    #[serde(rename_all = "camelCase")]
    ConfigChanged {
        version: u64,
        summary: String,
        timestamp_ms: i64,
    },

    #[serde(rename_all = "camelCase")]
    Activity {
        kind: String,
        message: String,
        timestamp_ms: i64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_tagged_with_a_camel_case_type() {
        let changed = serde_json::to_value(Event::ConfigChanged {
            version: 7,
            summary: "route created".into(),
            timestamp_ms: 5,
        })
        .unwrap();
        assert_eq!(changed["type"], "configChanged");
        assert_eq!(changed["version"], 7);
        assert_eq!(changed["summary"], "route created");
        assert_eq!(changed["timestampMs"], 5);

        let hello = serde_json::to_value(Event::Hello {
            message: "hi".into(),
            timestamp_ms: 1,
        })
        .unwrap();
        assert_eq!(hello["type"], "hello");
    }
}
