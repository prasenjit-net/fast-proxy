//! Forwards endpoint health and ejection changes to the UI.

use fast_proxy_telemetry::{EndpointChange, EndpointEvent};
use tokio::sync::broadcast::error::RecvError;

use crate::events::Event;
use crate::state::SharedState;

/// What the UI is told about one change: its name, the detail, and a line
/// for the activity feed.
fn describe(event: &EndpointEvent) -> (&'static str, Option<String>, String) {
    let who = format!(
        "endpoint {} of {} ({})",
        event.endpoint, event.upstream, event.subsystem
    );
    match &event.change {
        EndpointChange::Unhealthy { reason } => (
            "unhealthy",
            Some(reason.clone()),
            format!("{who} is unhealthy: {reason}"),
        ),
        EndpointChange::Healthy => ("healthy", None, format!("{who} is healthy again")),
        EndpointChange::Ejected { times, .. } => (
            "ejected",
            Some(format!("ejection number {times} in a row")),
            format!("{who} was ejected after repeated failures"),
        ),
        EndpointChange::Recovered => ("recovered", None, format!("{who} recovered")),
    }
}

/// Turns every endpoint event into an `endpointState` event and an activity
/// feed entry for connected clients.
pub fn spawn_endpoint_bridge(state: SharedState) {
    let mut events = state.endpoint_events.subscribe();
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let (change, detail, message) = describe(&event);
                    state.broadcast(Event::EndpointState {
                        subsystem: event.subsystem,
                        upstream: event.upstream,
                        endpoint: event.endpoint,
                        change,
                        detail,
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    });
                    state.activity("health", message);
                }
                // Missed events are only history; keep going.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(change: EndpointChange) -> EndpointEvent {
        EndpointEvent {
            subsystem: "shop".into(),
            upstream: "backend".into(),
            endpoint: "a:1".into(),
            change,
        }
    }

    #[test]
    fn every_change_has_a_name_and_a_readable_line() {
        let cases = [
            (
                EndpointChange::Unhealthy {
                    reason: "status 500".into(),
                },
                "unhealthy",
                "is unhealthy: status 500",
            ),
            (EndpointChange::Healthy, "healthy", "is healthy again"),
            (
                EndpointChange::Ejected {
                    until_ms: 5,
                    times: 2,
                },
                "ejected",
                "was ejected",
            ),
            (EndpointChange::Recovered, "recovered", "recovered"),
        ];
        for (change, name, line) in cases {
            let (got, _, message) = describe(&event(change));
            assert_eq!(got, name);
            assert!(message.contains(line), "{message}");
            assert!(
                message.contains("a:1") && message.contains("backend") && message.contains("shop")
            );
        }
    }

    #[test]
    fn details_explain_unhealthy_and_ejected_changes() {
        let (_, detail, _) = describe(&event(EndpointChange::Unhealthy {
            reason: "no answer".into(),
        }));
        assert_eq!(detail.as_deref(), Some("no answer"));
        let (_, detail, _) = describe(&event(EndpointChange::Ejected {
            until_ms: 1,
            times: 3,
        }));
        assert_eq!(detail.as_deref(), Some("ejection number 3 in a row"));
        assert_eq!(describe(&event(EndpointChange::Healthy)).1, None);
    }
}
