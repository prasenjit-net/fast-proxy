//! Forwards applied configuration changes to the UI.

use tokio::sync::broadcast::error::RecvError;

use crate::events::Event;
use crate::state::SharedState;

/// Turns every [`ConfigChanged`](fast_proxy_config_service::ConfigChanged)
/// from the config service into a `configChanged` event and an activity
/// feed entry for connected clients.
pub fn spawn_config_bridge(state: SharedState) {
    let mut changes = state.config.subscribe();
    tokio::spawn(async move {
        loop {
            match changes.recv().await {
                Ok(change) => {
                    state.broadcast(Event::ConfigChanged {
                        version: change.version,
                        summary: change.summary.clone(),
                        timestamp_ms: chrono::Utc::now().timestamp_millis(),
                    });
                    state.activity("config", change.summary);
                }
                // Missed events are only history; keep going.
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}
