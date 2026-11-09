use std::net::SocketAddr;

use serde::Deserialize;

/// The `[admin]` section: where the admin plane (UI, REST, `/metrics`) listens.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AdminConfig {
    pub listen: SocketAddr,
}

impl Default for AdminConfig {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([127, 0, 0, 1], 9000)),
        }
    }
}
