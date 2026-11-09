use thiserror::Error;

/// Why the config file could not be loaded or is unusable.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        source: std::io::Error,
    },
    #[error("invalid config {path}: {source}")]
    Parse {
        path: String,
        source: toml::de::Error,
    },
    /// The file parsed but its values are inconsistent or unusable.
    #[error("invalid config: {0}")]
    Invalid(String),
}
