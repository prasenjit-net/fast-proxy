//! Route table for the admin plane.

use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use fast_proxy_model::Action::{AuditExport, Keys, Read, Samples, Tokens, Write};

use crate::guard::Access::{Needs, Open, Ticket};
use crate::route_table::{RouteSpec, RouteTable};
use crate::state::SharedState;
use crate::{access_middleware, api, host_check, security_headers, static_assets, ws};

/// Builds the admin router: REST API, `/ws`, and the SPA fallback.
pub fn router(state: SharedState) -> Router {
    build(state).0
}

/// The router and the list of its routes with the access each needs.
pub(crate) fn build(state: SharedState) -> (Router, Vec<RouteSpec>) {
    let (routes, specs) = RouteTable::new(state.clone())
        .get("/api/health", Open, api::system::health)
        .get("/api/config", Needs(Read), api::system::config)
        .get("/api/traffic", Needs(Read), api::system::traffic)
        .get("/metrics", Needs(Read), api::system::prometheus)
        .get("/api/whoami", Needs(Read), api::whoami)
        .post("/api/ws-ticket", Needs(Read), api::ws_ticket)
        .get("/api/tokens", Needs(Tokens), api::list_tokens)
        .post("/api/tokens", Needs(Tokens), api::create_token)
        .delete("/api/tokens/{id}", Needs(Tokens), api::revoke_token)
        .get("/api/subsystems", Needs(Read), api::list_subsystems)
        .post("/api/subsystems", Needs(Write), api::create_subsystem)
        .get("/api/subsystems/{id}", Needs(Read), api::get_subsystem)
        .put("/api/subsystems/{id}", Needs(Write), api::update_subsystem)
        .delete("/api/subsystems/{id}", Needs(Write), api::delete_subsystem)
        .post("/api/subsystems/{id}/hosts", Needs(Write), api::add_host)
        .delete(
            "/api/subsystems/{id}/hosts/{host_id}",
            Needs(Write),
            api::remove_host,
        )
        .get(
            "/api/subsystems/{id}/upstreams",
            Needs(Read),
            api::list_upstreams,
        )
        .post(
            "/api/subsystems/{id}/upstreams",
            Needs(Write),
            api::create_upstream,
        )
        .put(
            "/api/subsystems/{id}/upstreams/{upstream_id}",
            Needs(Write),
            api::update_upstream,
        )
        .delete(
            "/api/subsystems/{id}/upstreams/{upstream_id}",
            Needs(Write),
            api::delete_upstream,
        )
        .get(
            "/api/subsystems/{id}/upstreams/{upstream_id}/status",
            Needs(Read),
            api::upstream_status,
        )
        .post(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints",
            Needs(Write),
            api::add_endpoint,
        )
        .put(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}",
            Needs(Write),
            api::update_endpoint,
        )
        .delete(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}",
            Needs(Write),
            api::delete_endpoint,
        )
        .put(
            "/api/subsystems/{id}/upstreams/{upstream_id}/endpoints/{endpoint_id}/state",
            Needs(Write),
            api::set_endpoint_state,
        )
        .get("/api/subsystems/{id}/routes", Needs(Read), api::list_routes)
        .post(
            "/api/subsystems/{id}/routes",
            Needs(Write),
            api::create_route,
        )
        .put(
            "/api/subsystems/{id}/routes/{route_id}",
            Needs(Write),
            api::update_route,
        )
        .delete(
            "/api/subsystems/{id}/routes/{route_id}",
            Needs(Write),
            api::delete_route,
        )
        .post(
            "/api/subsystems/{id}/certificate",
            Needs(Keys),
            api::upload_subsystem_certificate,
        )
        .delete(
            "/api/subsystems/{id}/certificate",
            Needs(Write),
            api::detach_subsystem_certificate,
        )
        .get("/api/certificates", Needs(Read), api::list_certificates)
        .post("/api/certificates", Needs(Keys), api::upload_certificate)
        .post(
            "/api/certificates/self-signed",
            Needs(Keys),
            api::generate_self_signed,
        )
        // A key in the body is checked in the handler, so a rename needs none.
        .put(
            "/api/certificates/{id}",
            Needs(Write),
            api::replace_certificate,
        )
        .delete(
            "/api/certificates/{id}",
            Needs(Write),
            api::delete_certificate,
        )
        .get("/api/ca-bundles", Needs(Read), api::list_ca_bundles)
        .post("/api/ca-bundles", Needs(Write), api::upload_ca_bundle)
        .delete("/api/ca-bundles/{id}", Needs(Write), api::delete_ca_bundle)
        .get("/api/tls", Needs(Read), api::tls_status)
        .get("/api/revisions", Needs(Read), api::list_revisions)
        .get("/api/revisions/{version}", Needs(Read), api::get_revision)
        .post(
            "/api/revisions/{version}/rollback",
            Needs(Write),
            api::rollback_revision,
        )
        .post("/api/resolve", Needs(Read), api::resolve)
        .get("/api/status", Needs(Read), api::status)
        .post("/api/reload", Needs(Write), api::reload)
        .get("/api/audit", Needs(Read), api::list_audit)
        .get("/api/audit/export", Needs(AuditExport), api::export_audit)
        .get("/api/sampling", Needs(Read), api::get_sampling)
        .put("/api/sampling", Needs(Write), api::put_sampling)
        .put("/api/sampling/overrides", Needs(Write), api::put_override)
        .delete(
            "/api/sampling/overrides",
            Needs(Write),
            api::delete_override,
        )
        .get("/api/samples", Needs(Samples), api::list_samples)
        .get("/api/samples/export", Needs(Samples), api::export_samples)
        .get("/api/samples/{id}", Needs(Samples), api::get_sample)
        .delete("/api/samples", Needs(Samples), api::clear_samples)
        .get("/api/maintenance", Needs(Read), api::list_maintenance)
        .put("/api/maintenance/global", Needs(Write), api::set_global)
        .delete("/api/maintenance/global", Needs(Write), api::clear_global)
        .put(
            "/api/subsystems/{id}/maintenance",
            Needs(Write),
            api::set_subsystem,
        )
        .delete(
            "/api/subsystems/{id}/maintenance",
            Needs(Write),
            api::clear_subsystem,
        )
        .put(
            "/api/subsystems/{id}/routes/{route_id}/maintenance",
            Needs(Write),
            api::set_route,
        )
        .delete(
            "/api/subsystems/{id}/routes/{route_id}/maintenance",
            Needs(Write),
            api::clear_route,
        )
        .get("/ws", Ticket(Read), ws::handler)
        .finish();
    // Layers run outermost first: the access log sees every response, the
    // security headers go on whatever comes back (a refusal included), and
    // the Host check runs before anything else looks at the request.
    let router = routes
        .fallback(static_assets::handler)
        .layer(from_fn_with_state(state.clone(), host_check::check))
        .layer(from_fn(security_headers::add))
        .layer(from_fn_with_state(state.clone(), access_middleware::record))
        .with_state(state);
    (router, specs)
}
