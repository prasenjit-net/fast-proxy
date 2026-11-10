//! REST handlers, one file per resource.

mod config_view;
mod detail;
mod effective_state;
mod endpoints;
mod envelope;
mod hosts;
mod input;
mod json;
mod resolve;
mod revisions;
mod routes;
mod status;
mod subsystems;
pub mod system;
mod upstream_dto;
mod upstream_health;
mod upstream_status;
mod upstreams;

pub use endpoints::{add_endpoint, delete_endpoint, set_endpoint_state, update_endpoint};
pub use hosts::{add_host, remove_host};
pub use resolve::resolve;
pub use revisions::{get_revision, list_revisions, rollback_revision};
pub use routes::{create_route, delete_route, list_routes, update_route};
pub use status::status;
pub use subsystems::{
    create_subsystem, delete_subsystem, get_subsystem, list_subsystems, update_subsystem,
};
pub use upstream_status::upstream_status;
pub use upstreams::{create_upstream, delete_upstream, list_upstreams, update_upstream};
