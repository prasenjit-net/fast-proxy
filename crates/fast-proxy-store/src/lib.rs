//! SQLite persistence for subsystems, hosts, upstreams, routes, certificates, and revision history.
//!
//! [`Database`] is the synchronous core; [`Store`] runs one on a dedicated
//! thread for async callers. All writes go through [`Database::write`], which
//! validates the would-be configuration before committing and records a revision.

mod certificate_source;
mod clock;
mod database;
mod error;
mod inputs;
pub mod repository;
pub mod revision;
mod store;

#[cfg(test)]
mod certificate_source_tests;
#[cfg(test)]
mod test_support;

pub use certificate_source::CertificateSource;
pub use database::{Database, Mutation, Written};
pub use error::StoreError;
pub use inputs::{EndpointFields, RouteFields, SubsystemFields, UpstreamFields};
pub use rusqlite::Connection;
pub use store::Store;
