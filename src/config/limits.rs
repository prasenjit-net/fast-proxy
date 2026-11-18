use std::time::Duration;

use fast_proxy_limits::defaults;
use serde::Deserialize;

use super::duration::deserialize_duration;

/// The `[limits]` section: process limits (ADR 0029), changed by a restart.
/// Rate-limit *policies* and per-route body caps are configuration in the
/// database; this section bounds what any request or client may cost the process.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// Open client connections across all listeners. `--max-connections`
    /// overrides it.
    pub max_connections: usize,
    /// Open connections from one peer address; 0 turns the check off (behind a
    /// load balancer every peer is the balancer).
    pub max_connections_per_client: usize,
    /// How long a client may take to send a request head; also how long a
    /// keep-alive connection may sit idle between requests.
    #[serde(deserialize_with = "deserialize_duration")]
    pub header_read_timeout: Duration,
    /// Request header fields; more are refused with 431.
    pub max_header_count: usize,
    /// Bytes of request head; a larger one is refused with 431.
    pub max_header_bytes: usize,
    /// Bytes of request target (path and query); longer is refused with 414.
    pub max_uri_bytes: usize,
    /// The body cap of routes that set none; 0 is unlimited. Larger bodies get 413.
    pub max_body_bytes: u64,
    /// How long a request body may stall once the upstream reads it (then 408).
    #[serde(deserialize_with = "deserialize_duration")]
    pub body_read_timeout: Duration,
    /// How long writes to a client that stopped reading may stay blocked.
    #[serde(deserialize_with = "deserialize_duration")]
    pub send_timeout: Duration,
    /// How long a tunnel (WebSocket) may carry nothing either way; "0s" never
    /// closes an idle tunnel.
    #[serde(deserialize_with = "deserialize_duration")]
    pub tunnel_idle_timeout: Duration,
    /// How many client keys every rate-limit table together may hold. When it is
    /// reached, idle keys make room; if none is idle a new client is let through
    /// and counted (fail open), so the table cannot be turned against everyone.
    pub rate_limit_keys: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_connections: defaults::MAX_CONNECTIONS,
            max_connections_per_client: defaults::MAX_CONNECTIONS_PER_CLIENT,
            header_read_timeout: defaults::HEADER_READ_TIMEOUT,
            max_header_count: defaults::MAX_HEADER_COUNT,
            max_header_bytes: defaults::MAX_HEADER_BYTES,
            max_uri_bytes: defaults::MAX_URI_BYTES,
            max_body_bytes: 0,
            body_read_timeout: defaults::BODY_READ_TIMEOUT,
            send_timeout: defaults::SEND_TIMEOUT,
            tunnel_idle_timeout: defaults::TUNNEL_IDLE_TIMEOUT,
            rate_limit_keys: fast_proxy_limits::DEFAULT_KEY_CAPACITY,
        }
    }
}
