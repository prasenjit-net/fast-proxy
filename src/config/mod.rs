//! Config file model (`config.toml`): process-level settings only.
//! Subsystems, hosts, routes, and upstreams live in the database.

mod acme;
mod admin;
mod data_plane;
mod duration;
mod error;
mod logging;
mod sampling;
mod tls;
mod validate;
mod validate_acme;

use std::path::Path;

use fast_proxy_admin::UiConfig;
use serde::Deserialize;

pub use acme::{AcmeConfig, LETS_ENCRYPT_STAGING};
pub use admin::{AdminAuth, AdminConfig, AdminTlsConfig};
pub use data_plane::DataPlaneConfig;
pub use duration::parse_duration;
pub use error::ConfigError;
pub use logging::LoggingConfig;
pub use sampling::SamplingConfig;
pub use tls::TlsConfig;

/// Everything the config file can set.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub data_plane: DataPlaneConfig,
    pub admin: AdminConfig,
    pub logging: LoggingConfig,
    pub sampling: SamplingConfig,
    pub ui: UiConfig,
}

impl AppConfig {
    /// Loads the file at `path`, or built-in defaults (with a warning) if it is missing.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            eprintln!(
                "warning: config file {} not found, using built-in defaults",
                path.display()
            );
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.display().to_string(),
            source,
        })?;
        toml::from_str(&raw).map_err(|source| ConfigError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    /// Checks the final, override-applied configuration without touching the disk.
    pub fn check(&self) -> Result<(), ConfigError> {
        validate::check(self)
    }

    /// What an operator should know about the ACME setup (staging, port 80);
    /// printed at startup, never fatal.
    pub fn acme_warnings(&self) -> Vec<String> {
        validate_acme::warnings(self)
    }

    /// Checks the final, override-applied configuration and prepares the state directory.
    pub fn validate(&self) -> Result<(), ConfigError> {
        validate::validate(self)
    }
}

#[cfg(test)]
mod tests;
