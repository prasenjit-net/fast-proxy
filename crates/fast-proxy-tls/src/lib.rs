//! Everything about certificates, keys and rustls configurations; no HTTP
//! knowledge (ADR 0010, 0011).
//!
//! - [`validate_upload`] parses and checks PEM supplied by an admin.
//! - [`Vault`] seals private keys with AES-256-GCM under a master key; the
//!   store only ever sees ciphertext.
//! - [`generate_self_signed`] makes development certificates; the ephemeral
//!   "unknown host" certificate is made in memory and never stored.
//! - [`CertCache`] holds parsed, decrypted keys and reuses unchanged entries
//!   across rebuilds.
//! - [`build_server_config`] builds the rustls server configuration from the TLS policy.
//! - [`ClientConfigs`] builds rustls client configurations for upstream TLS.

mod cache;
mod client_config;
mod key_material;
mod self_signed;
mod server_config;
mod status;
mod validate;
mod vault;

pub use cache::{
    CertCache, CertEntry, CertHandle, CertVersion, RebuildStats, StoredCertificate, UnreadableCert,
    UnreadableReason,
};
pub use client_config::{
    server_name_for, CaBundle, ClientConfigError, ClientConfigs, UpstreamTrust,
};
pub use key_material::{KeyKind, KeyMaterial};
pub use self_signed::{ephemeral_unknown_host, generate_self_signed, GeneratedCertificate};
pub use server_config::{build_server_config, MinTlsVersion, ServerConfigError, ServerTlsPolicy};
pub use status::{certificate_readings, CertificateState, EXPIRING_SOON_DAYS};
pub use validate::{
    expiry_state, parse_chain, parse_key, read_metadata, validate_upload, ExpiryState,
    ValidatedCertificate,
};
pub use vault::{load_master_key, LoadedMasterKey, MasterKey, Sealed, Vault, VaultError};
