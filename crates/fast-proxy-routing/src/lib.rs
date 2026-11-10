//! Compiled routing snapshot plus the host and route matchers. Pure: no I/O.
//!
//! The admin side turns a [`fast_proxy_model::Config`] into an immutable
//! [`Snapshot`] with [`compile`]; the data plane only ever reads snapshots
//! through a [`SnapshotHolder`] and never touches storage.

mod compile;
mod compile_error;
mod compiled_https;
mod compiled_upstream;
mod holder;
mod host_table;
mod lookup;
mod route_table;
mod snapshot;
mod tls_inputs;
mod upstream_uri;

pub use compile::{compile, compile_with_tls};
pub use compile_error::CompileError;
pub use compiled_https::CompiledHttps;
pub use compiled_upstream::{CompiledEndpoint, CompiledUpstream};
pub use holder::SnapshotHolder;
pub use host_table::HostTable;
pub use lookup::{Lookup, SniLookup};
pub use route_table::{RouteMatch, RouteTable};
pub use snapshot::{CompiledRoute, CompiledSubsystem, Snapshot};
pub use tls_inputs::TlsInputs;
pub use upstream_uri::build_upstream_uri;

#[cfg(test)]
mod tls_compile_tests;
