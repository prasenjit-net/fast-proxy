use std::path::PathBuf;
use std::time::Duration;

use fast_proxy_model::AcmeKeyType;
use serde::{Deserialize, Deserializer};

use super::duration::deserialize_duration;

/// Let's Encrypt's staging directory: the default, so a first setup cannot
/// spend production rate limits. Its certificates are not publicly trusted.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// The `[data_plane.acme]` section: the process-wide ACME switch, the CA and
/// the names it may issue for. Which subsystems use ACME is in the database
/// (`https.acmeEnabled`); certificates and the account key are stored there too
/// (ADR 0014).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AcmeConfig {
    /// Turns ACME on for the process; needs `[data_plane.tls] enabled`.
    /// `--acme` and `--no-acme` override it.
    pub enabled: bool,
    /// The ACME directory URL. Production must be written out.
    pub directory: String,
    /// The account contact; optional.
    pub email: String,
    /// The names ACME may issue for: `example.com` or `*.example.com` (any name
    /// below it). Required when ACME is on.
    pub allowed_domains: Vec<String>,
    /// The key algorithm of issued certificates.
    #[serde(deserialize_with = "deserialize_key_type")]
    pub key_type: AcmeKeyType,
    /// A PEM file of roots to trust for the directory (a private CA, Pebble).
    pub directory_ca: Option<PathBuf>,
    /// How often certificates are checked for renewal.
    #[serde(deserialize_with = "deserialize_duration")]
    pub renew_check_interval: Duration,
    /// How long to wait for other instances to see a challenge before the CA is
    /// asked to check it; defaults to twice the config poll interval plus 2s.
    #[serde(deserialize_with = "deserialize_optional_duration")]
    pub propagation_wait: Option<Duration>,
    /// How long the renewal lease lasts without being renewed.
    #[serde(deserialize_with = "deserialize_duration")]
    pub lease_ttl: Duration,
    /// Before the CA is asked to validate, fetch each challenge through its
    /// public name, so a DNS or firewall problem costs no CA attempt.
    pub self_check: bool,
}

impl Default for AcmeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            directory: LETS_ENCRYPT_STAGING.into(),
            email: String::new(),
            allowed_domains: Vec::new(),
            key_type: AcmeKeyType::EcdsaP256,
            directory_ca: None,
            renew_check_interval: Duration::from_secs(3600),
            propagation_wait: None,
            lease_ttl: Duration::from_secs(600),
            self_check: true,
        }
    }
}

fn deserialize_key_type<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<AcmeKeyType, D::Error> {
    let raw = String::deserialize(deserializer)?;
    AcmeKeyType::parse(&raw).map_err(|err| serde::de::Error::custom(err.message))
}

fn deserialize_optional_duration<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Duration>, D::Error> {
    deserialize_duration(deserializer).map(Some)
}
