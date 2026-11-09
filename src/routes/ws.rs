use std::sync::atomic::Ordering;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;

use crate::services::events::Event;
use crate::state::SharedState;

/// `GET /ws` — upgrade and stream server-push events to the client.
pub async fn handler(ws: WebSocketUpgrade, State(state): State<SharedState>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state))
}

async fn serve(mut socket: WebSocket, state: SharedState) {
    let online = state.ws_clients.fetch_add(1, Ordering::Relaxed) + 1;
    state.activity(
        "socket",
        format!("WebSocket client connected ({online} online)"),
    );

    stream(&mut socket, &state).await;

    let online = state.ws_clients.fetch_sub(1, Ordering::Relaxed) - 1;
    state.activity(
        "socket",
        format!("WebSocket client disconnected ({online} online)"),
    );
}

async fn stream(socket: &mut WebSocket, state: &SharedState) {
    let mut events = state.events.subscribe();

    let hello = Event::Hello {
        message: format!(
            "Connected to {} v{}",
            state.config.ui.app_name,
            env!("CARGO_PKG_VERSION")
        ),
        timestamp_ms: chrono::Utc::now().timestamp_millis(),
    };
    if send(socket, &hello).await.is_err() {
        return;
    }

    // Seed the dashboard immediately instead of waiting for the next tick.
    if let Some(snapshot) = state.latest_metrics.read().await.clone() {
        if send(socket, &Event::Metrics { data: snapshot })
            .await
            .is_err()
        {
            return;
        }
    }

    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                // Client hung up (or errored) — stop streaming.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                // Pings are answered by axum automatically; ignore the rest.
                Some(Ok(_)) => {}
            },
            event = events.recv() => match event {
                Ok(event) => {
                    if send(socket, &event).await.is_err() {
                        return;
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!("slow ws client skipped {skipped} events");
                }
                Err(RecvError::Closed) => return,
            },
        }
    }
}

async fn send(socket: &mut WebSocket, event: &Event) -> Result<(), axum::Error> {
    let json = serde_json::to_string(event).expect("events always serialize");
    socket.send(Message::Text(json.into())).await
}
