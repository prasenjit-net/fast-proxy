//! Forwards the ACME statuses this instance writes to the UI.

use tokio::sync::broadcast::error::RecvError;

use crate::events::Event;
use crate::state::SharedState;

/// Turns every status the runner records into an `acmeStatus` event.
pub fn spawn_acme_bridge(state: SharedState) {
    let mut statuses = state.config.subscribe_acme();
    tokio::spawn(async move {
        loop {
            match statuses.recv().await {
                Ok(status) => state.broadcast(Event::AcmeStatus {
                    subsystem_id: status.subsystem_id.0,
                    state: status.state.as_str(),
                    next_attempt_at_ms: status.next_attempt_at_ms,
                    last_error: status.last_error,
                    timestamp_ms: crate::clock::now_ms(),
                }),
                Err(RecvError::Lagged(_)) => continue,
                Err(RecvError::Closed) => return,
            }
        }
    });
}
