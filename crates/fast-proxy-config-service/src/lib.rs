//! The config apply pipeline. Every admin change goes through
//! [`ConfigService::apply`], which validates it against the compiled routing
//! rules, persists it with a revision, and swaps the live snapshot, in that
//! order and under one writer lock. The data plane only ever sees the holder.

mod certificates;
mod dev_certificates;
mod error;
mod metric_series;
mod service;
mod tls_state;

pub use certificates::{CertificateChange, CertificateSummary, UploadOutcome};
pub use error::StartError;
pub use service::{Applied, ConfigChanged, ConfigService};
pub use tls_state::{TlsSettings, VaultStatus};

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tls_tests;
