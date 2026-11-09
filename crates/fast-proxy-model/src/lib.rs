//! Domain types and pure validation for fast-proxy configuration. No I/O lives here.
//!
//! Every type that wraps user input is constructed through a `parse` function
//! that validates and normalizes it, so a value of that type is always valid.

mod config;
mod endpoint_addr;
mod field_error;
mod host_pattern;
mod path_prefix;
mod subsystem_name;
mod timeout;

pub use config::{
    Config, EndpointConfig, HostId, RouteConfig, RouteId, SubsystemConfig, SubsystemId,
    UpstreamGroupConfig, UpstreamGroupId,
};
pub use endpoint_addr::EndpointAddr;
pub use field_error::FieldError;
pub use host_pattern::HostPattern;
pub use path_prefix::{RoutePrefix, UpstreamPrefix};
pub use subsystem_name::SubsystemName;
pub use timeout::parse_timeout_ms;
