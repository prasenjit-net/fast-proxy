//! Route table for the admin plane.

use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::state::SharedState;
use crate::{access_middleware, api, static_assets, ws};

/// Builds the admin router: REST API, `/ws`, and the SPA fallback.
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(api::system::health))
        .route("/api/config", get(api::system::config))
        .route("/api/metrics", get(api::system::metrics))
        .route(
            "/api/subsystems",
            get(api::list_subsystems).post(api::create_subsystem),
        )
        .route(
            "/api/subsystems/{id}",
            get(api::get_subsystem)
                .put(api::update_subsystem)
                .delete(api::delete_subsystem),
        )
        .route("/api/subsystems/{id}/hosts", post(api::add_host))
        .route(
            "/api/subsystems/{id}/hosts/{host_id}",
            delete(api::remove_host),
        )
        .route(
            "/api/subsystems/{id}/upstreams",
            get(api::list_upstreams).post(api::create_upstream),
        )
        .route(
            "/api/subsystems/{id}/upstreams/{upstream_id}",
            put(api::update_upstream).delete(api::delete_upstream),
        )
        .route(
            "/api/subsystems/{id}/routes",
            get(api::list_routes).post(api::create_route),
        )
        .route(
            "/api/subsystems/{id}/routes/{route_id}",
            put(api::update_route).delete(api::delete_route),
        )
        .route("/ws", get(ws::handler))
        .fallback(static_assets::handler)
        .layer(from_fn_with_state(state.clone(), access_middleware::record))
        .with_state(state)
}
