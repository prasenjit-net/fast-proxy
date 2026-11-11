//! REST handlers, one file per resource.

mod audit;
mod ca_bundles;
mod certificate_dto;
mod certificates;
mod change;
mod config_view;
mod detail;
mod effective_state;
mod endpoints;
mod envelope;
mod hosts;
mod https_dto;
mod input;
mod json;
mod maintenance;
mod reload;
mod resolve;
mod revisions;
mod routes;
mod session;
mod status;
mod subsystems;
pub mod system;
mod tls_status;
mod tokens;
mod upstream_dto;
mod upstream_health;
mod upstream_status;
mod upstreams;

pub use audit::{export_audit, list_audit};
pub use ca_bundles::{delete_ca_bundle, list_ca_bundles, upload_ca_bundle};
pub use certificates::{
    delete_certificate, detach_subsystem_certificate, generate_self_signed, list_certificates,
    replace_certificate, upload_certificate, upload_subsystem_certificate,
};
pub use endpoints::{add_endpoint, delete_endpoint, set_endpoint_state, update_endpoint};
pub use hosts::{add_host, remove_host};
pub use maintenance::{
    clear_global, clear_route, clear_subsystem, list_maintenance, set_global, set_route,
    set_subsystem,
};
pub use reload::reload;
pub use resolve::resolve;
pub use revisions::{get_revision, list_revisions, rollback_revision};
pub use routes::{create_route, delete_route, list_routes, update_route};
pub use session::{whoami, ws_ticket};
pub use status::status;
pub use subsystems::{
    create_subsystem, delete_subsystem, get_subsystem, list_subsystems, update_subsystem,
};
pub use tls_status::tls_status;
pub use tokens::{create_token, list_tokens, revoke_token};
pub use upstream_status::upstream_status;
pub use upstreams::{create_upstream, delete_upstream, list_upstreams, update_upstream};
