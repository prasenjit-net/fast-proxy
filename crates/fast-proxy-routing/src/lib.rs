//! Compiled routing snapshot plus the host and route matchers. Pure: no I/O.
//!
//! The admin side turns a [`fast_proxy_model::Config`] into an immutable
//! [`Snapshot`] with [`compile`]; the data plane only ever reads snapshots
//! through a [`SnapshotHolder`] and never touches storage.

mod compile;
mod compile_error;
mod compiled_https;
mod compiled_maintenance;
mod compiled_sampling;
mod compiled_upstream;
mod holder;
mod host_table;
mod lookup;
mod maintenance_inputs;
mod redirect;
mod route_table;
mod sampling_inputs;
mod sampling_table;
mod snapshot;
mod tls_inputs;
mod upstream_uri;

pub use compile::{compile, compile_all, compile_full, compile_with_tls};
pub use compile_error::CompileError;
pub use compiled_https::CompiledHttps;
pub use compiled_maintenance::{
    CompiledMaintenance, MaintenanceCheck, MaintenanceScope, RenderedBody, ResponseFormat,
};
pub use compiled_sampling::CompiledSampling;
pub use compiled_upstream::{CompiledEndpoint, CompiledUpstream};
pub use holder::SnapshotHolder;
pub use host_table::{dns_host, HostTable};
pub use lookup::{Lookup, SniLookup};
pub use maintenance_inputs::MaintenanceInputs;
pub use redirect::redirect_target;
pub use route_table::{RouteMatch, RouteTable};
pub use sampling_inputs::{SamplingInputs, SamplingRuntime};
pub use snapshot::{CompiledRoute, CompiledSubsystem, Snapshot};
pub use tls_inputs::TlsInputs;
pub use upstream_uri::build_upstream_uri;

#[cfg(test)]
mod maintenance_compile_tests;
#[cfg(test)]
mod sampling_compile_tests;
#[cfg(test)]
mod tls_compile_tests;
