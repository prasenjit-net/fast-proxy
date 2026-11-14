//! The config apply pipeline. Every admin change goes through
//! [`ConfigService::apply`], which validates it against the compiled routing
//! rules, persists it with a revision, and swaps the live snapshot, in that
//! order and under one writer lock. The data plane only ever sees the holder.

mod audit_events;
mod audit_retention;
mod ca_bundles;
mod certificates;
mod change_context;
mod dev_certificates;
mod error;
mod maintenance;
mod metric_series;
mod sampling;
mod service;
mod tls_state;
mod tokens;

pub use audit_events::AuditEvent;
pub use audit_retention::AuditRetention;
pub use ca_bundles::CaBundleSummary;
pub use certificates::{CertificateChange, CertificateSummary, SubsystemEdit, UploadOutcome};
pub use change_context::{Audited, ChangeContext};
pub use error::StartError;
pub use service::{Applied, ConfigChanged, ConfigService};
pub use tls_state::{TlsSettings, VaultStatus};
pub use tokens::{Bootstrap, CreatedToken};

#[cfg(test)]
mod operations_tests;
#[cfg(test)]
mod sampling_tests;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tls_tests;
