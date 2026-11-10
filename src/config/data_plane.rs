use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Deserializer};

use super::TlsConfig;

/// The `[data_plane]` section: listeners and process-level proxy settings.
/// Subsystems, hosts, and routes are not here; they live in the database.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DataPlaneConfig {
    /// Directory for the database and other persistent state.
    pub state_dir: PathBuf,
    /// Database file; defaults to `<state_dir>/fast-proxy.db`.
    pub database: Option<PathBuf>,
    /// CIDRs whose `X-Forwarded-*` headers are trusted and appended to.
    pub trusted_proxies: Vec<String>,
    /// How long in-flight requests may finish after a shutdown signal.
    #[serde(deserialize_with = "deserialize_duration")]
    pub shutdown_grace: Duration,
    #[serde(default = "default_listeners")]
    pub listener: Vec<ListenerConfig>,
    /// The master key file for sealing private keys; defaults to
    /// `<state_dir>/master.key`. `FAST_PROXY_MASTER_KEY` overrides it.
    pub master_key_file: Option<PathBuf>,
    /// How often to check the database for changes made by another instance;
    /// `0s` turns the cluster sync off.
    #[serde(deserialize_with = "deserialize_duration")]
    pub config_poll_interval: Duration,
    /// TLS termination.
    pub tls: TlsConfig,
}

/// One `[[data_plane.listener]]` entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListenerConfig {
    pub name: String,
    pub listen: SocketAddr,
}

impl DataPlaneConfig {
    /// The database file to open, applying the `<state_dir>` default.
    pub fn database_path(&self) -> PathBuf {
        self.database
            .clone()
            .unwrap_or_else(|| self.state_dir.join("fast-proxy.db"))
    }
}

impl DataPlaneConfig {
    /// The master key file, applying the `<state_dir>` default.
    pub fn master_key_path(&self) -> PathBuf {
        self.master_key_file
            .clone()
            .unwrap_or_else(|| self.state_dir.join("master.key"))
    }
}

impl Default for DataPlaneConfig {
    fn default() -> Self {
        Self {
            state_dir: PathBuf::from("./state"),
            database: None,
            trusted_proxies: Vec::new(),
            shutdown_grace: Duration::from_secs(30),
            listener: default_listeners(),
            master_key_file: None,
            config_poll_interval: Duration::from_secs(5),
            tls: TlsConfig::default(),
        }
    }
}

/// A single plain-HTTP listener, used when the file defines none.
fn default_listeners() -> Vec<ListenerConfig> {
    vec![ListenerConfig {
        name: "http".into(),
        listen: SocketAddr::from(([0, 0, 0, 0], 8000)),
    }]
}

fn deserialize_duration<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Duration, D::Error> {
    let raw = String::deserialize(deserializer)?;
    humantime::parse_duration(&raw).map_err(serde::de::Error::custom)
}
