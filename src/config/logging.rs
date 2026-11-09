use std::path::PathBuf;

use serde::Deserialize;

/// The `[logging]` section.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// A `tracing` filter, e.g. `info` or `info,access=warn`.
    pub level: String,
    /// Access-log file; `None` logs to the console only.
    pub access_log: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            access_log: None,
        }
    }
}
