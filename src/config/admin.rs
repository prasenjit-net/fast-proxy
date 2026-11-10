use std::net::SocketAddr;

use clap::ValueEnum;
use serde::Deserialize;

/// Whether the admin plane asks for a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum AdminAuth {
    /// Every API route and the WebSocket need a valid token (the default).
    Required,
    /// No token is asked for; accepted only while the listener is on loopback.
    Off,
}

/// The `[admin]` section: where the admin plane (UI, REST, `/metrics`) listens
/// and how it is protected.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    pub listen: SocketAddr,
    pub auth: AdminAuth,
    /// Extra names accepted in the `Host` header; the listen address and the
    /// loopback names are always accepted.
    pub allowed_hosts: Vec<String>,
    /// Accept a non-loopback listener without TLS (tokens then cross the
    /// network in clear text). Authentication stays required.
    pub allow_insecure_remote: bool,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 9000)),
            auth: AdminAuth::Required,
            allowed_hosts: Vec::new(),
            allow_insecure_remote: false,
        }
    }
}
