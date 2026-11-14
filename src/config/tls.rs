use std::net::SocketAddr;
use std::time::Duration;

use fast_proxy_tls::MinTlsVersion;
use serde::{Deserialize, Deserializer};

use super::duration::deserialize_duration;

/// The `[data_plane.tls]` section: the process-wide TLS switch and policy.
/// There are no certificate or key entries: certificates live in the database
/// and are managed in the admin UI/API (ADR 0010).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TlsConfig {
    /// Starts the HTTPS listener. Off by default; `--tls` / `--no-tls` override it.
    pub enabled: bool,
    /// The HTTPS listener's address (`--https-listen`); use port 443 in production.
    pub listen: SocketAddr,
    /// The port redirects point at (NAT, containers); defaults to `listen`'s port.
    pub public_port: Option<u16>,
    /// The lowest TLS version accepted: `"1.2"` or `"1.3"`.
    #[serde(deserialize_with = "deserialize_min_version")]
    pub min_version: MinTlsVersion,
    /// Offer HTTP/2 through ALPN.
    pub http2: bool,
    /// Fail the handshake for an unknown name instead of presenting a placeholder certificate.
    pub reject_unknown_sni: bool,
    /// How long a handshake may take before the connection is dropped.
    #[serde(deserialize_with = "deserialize_duration")]
    pub handshake_timeout: Duration,
    /// How many handshakes may run at once.
    pub max_concurrent_handshakes: usize,
    /// Concurrent HTTP/2 streams one client connection may open.
    pub http2_max_concurrent_streams: u32,
    /// The largest HTTP/2 request header list accepted, in bytes.
    pub http2_max_header_list_size: u32,
    /// How often an idle HTTP/2 connection is pinged.
    #[serde(deserialize_with = "deserialize_duration")]
    pub http2_keep_alive_interval: Duration,
    /// How long an HTTP/2 ping may go unanswered before the connection closes.
    #[serde(deserialize_with = "deserialize_duration")]
    pub http2_keep_alive_timeout: Duration,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: SocketAddr::from(([0, 0, 0, 0], 8443)),
            public_port: None,
            min_version: MinTlsVersion::V1_2,
            http2: true,
            reject_unknown_sni: false,
            handshake_timeout: Duration::from_secs(10),
            max_concurrent_handshakes: 256,
            http2_max_concurrent_streams: 128,
            http2_max_header_list_size: 64 * 1024,
            http2_keep_alive_interval: Duration::from_secs(30),
            http2_keep_alive_timeout: Duration::from_secs(20),
        }
    }
}

fn deserialize_min_version<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<MinTlsVersion, D::Error> {
    let raw = String::deserialize(deserializer)?;
    MinTlsVersion::parse(&raw)
        .ok_or_else(|| serde::de::Error::custom("min_version must be \"1.2\" or \"1.3\""))
}
