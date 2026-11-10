//! The read boundary for certificate material. The config service reads
//! certificates only through this trait, so a PostgreSQL implementation can
//! replace SQLite later without touching it (ADR 0011).
//!
//! Portability checklist for the certificate SQL (the tables `certificate`,
//! `ca_bundle` and `vault`): `BLOB` maps to `BYTEA`; timestamps are millisecond
//! `BIGINT`; the SAN list is JSON in `TEXT`; ids are plain integers; the only
//! upsert is the standard `ON CONFLICT (id) DO UPDATE`; no SQLite-only functions
//! or pragmas appear in these queries.

use crate::repository::ca_bundles::CaBundleRow;
use crate::repository::certificates::CertificateRow;
use crate::repository::vault::VaultCheck;
use crate::repository::{ca_bundles, certificates, vault};
use crate::{Database, StoreError};

/// Reads certificate material and the cluster generation.
pub trait CertificateSource {
    /// Every stored certificate, ordered by id.
    fn certificates(&self) -> Result<Vec<CertificateRow>, StoreError>;
    /// Every stored CA bundle, ordered by id.
    fn ca_bundles(&self) -> Result<Vec<CaBundleRow>, StoreError>;
    /// The stored vault check value, if any.
    fn vault_check(&self) -> Result<Option<VaultCheck>, StoreError>;
    /// The revision version: the cluster generation every committed change bumps.
    fn version(&self) -> Result<u64, StoreError>;
}

impl CertificateSource for Database {
    fn certificates(&self) -> Result<Vec<CertificateRow>, StoreError> {
        self.read(certificates::list)
    }

    fn ca_bundles(&self) -> Result<Vec<CaBundleRow>, StoreError> {
        self.read(ca_bundles::list)
    }

    fn vault_check(&self) -> Result<Option<VaultCheck>, StoreError> {
        self.read(vault::get)
    }

    fn version(&self) -> Result<u64, StoreError> {
        self.current_version()
    }
}
