//! REST handlers, one file per resource.

mod detail;
mod envelope;
mod hosts;
mod input;
mod json;
mod routes;
mod subsystems;
pub mod system;
mod upstreams;

pub use hosts::{add_host, remove_host};
pub use routes::{create_route, delete_route, list_routes, update_route};
pub use subsystems::{
    create_subsystem, delete_subsystem, get_subsystem, list_subsystems, update_subsystem,
};
pub use upstreams::{create_upstream, delete_upstream, list_upstreams, update_upstream};
