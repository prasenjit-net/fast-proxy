//! Route table for the admin plane.

use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::Router;

use crate::state::SharedState;
use crate::{access_middleware, api, static_assets, ws};

/// Builds the admin router: REST API, `/ws`, and the SPA fallback.
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(api::system::health))
        .route("/api/config", get(api::system::config))
        .route("/api/metrics", get(api::system::metrics))
        .route("/ws", get(ws::handler))
        .fallback(static_assets::handler)
        .layer(from_fn_with_state(state.clone(), access_middleware::record))
        .with_state(state)
}
