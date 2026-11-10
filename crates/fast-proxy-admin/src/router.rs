//! Route table for the admin plane.

use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post, put};
use axum::Router;

use crate::state::SharedState;
use crate::{access_middleware, api, static_assets, ws};

/// The most a request carrying certificate material may weigh: a 256 KiB chain,
/// a 256 KiB key and JSON around them.
const KEY_BODY_LIMIT: usize = 600 * 1024;

/// Builds the admin router: REST API, `/ws`, and the SPA fallback.
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(api::system::health))
        .route("/api/config", get(api::system::config))
        .route("/api/traffic", get(api::system::traffic))
        .route("/metrics", get(api::system::prometheus))
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
            "/api/subsystems/{id}/upstreams/{upstream_id}/status",
            get(api::upstream_status),
        )
        .route(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints",
            post(api::add_endpoint),
        )
        .route(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}",
            put(api::update_endpoint).delete(api::delete_endpoint),
        )
        .route(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}/state",
            put(api::set_endpoint_state),
        )
        .route(
            "/api/subsystems/{id}/routes",
            get(api::list_routes).post(api::create_route),
        )
        .route(
            "/api/subsystems/{id}/routes/{route_id}",
            put(api::update_route).delete(api::delete_route),
        )
        .route(
            "/api/subsystems/{id}/certificate",
            post(api::upload_subsystem_certificate)
                .delete(api::detach_subsystem_certificate)
                .layer(DefaultBodyLimit::max(KEY_BODY_LIMIT)),
        )
        .route(
            "/api/certificates",
            get(api::list_certificates)
                .post(api::upload_certificate)
                .layer(DefaultBodyLimit::max(KEY_BODY_LIMIT)),
        )
        .route(
            "/api/certificates/self-signed",
            post(api::generate_self_signed),
        )
        .route(
            "/api/certificates/{id}",
            put(api::replace_certificate)
                .delete(api::delete_certificate)
                .layer(DefaultBodyLimit::max(KEY_BODY_LIMIT)),
        )
        .route(
            "/api/ca-bundles",
            get(api::list_ca_bundles).post(api::upload_ca_bundle),
        )
        .route("/api/ca-bundles/{id}", delete(api::delete_ca_bundle))
        .route("/api/tls", get(api::tls_status))
        .route("/api/revisions", get(api::list_revisions))
        .route("/api/revisions/{version}", get(api::get_revision))
        .route(
            "/api/revisions/{version}/rollback",
            post(api::rollback_revision),
        )
        .route("/api/resolve", post(api::resolve))
        .route("/api/status", get(api::status))
        .route("/ws", get(ws::handler))
        .fallback(static_assets::handler)
        .layer(from_fn_with_state(state.clone(), access_middleware::record))
        .with_state(state)
}
