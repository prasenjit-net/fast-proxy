//! The process limits the data plane enforces (ADR 0029), from `[limits]` in
//! `config.toml`. Defaults come from [`fast_proxy_limits::defaults`].

use std::time::Duration;

use fast_proxy_limits::{defaults, ConnectionCaps};

/// Limits on connections and on what hyper reads of a request head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerLimits {
    /// Checked when a connection is accepted, before any TLS handshake.
    pub connections: ConnectionCaps,
    /// How long a client may take to send a request head; hyper also closes a
    /// keep-alive connection idle this long between requests.
    pub header_read_timeout: Duration,
    /// More header fields than this is refused with 431.
    pub max_header_count: usize,
    /// A request head larger than this is refused with 431.
    pub max_header_bytes: usize,
    /// How long writes to a client may stay blocked before it is closed.
    pub send_timeout: Duration,
    /// How long a tunnel may carry nothing either way; `None` never closes it.
    pub tunnel_idle_timeout: Option<Duration>,
}

impl Default for ServerLimits {
    fn default() -> Self {
        Self {
            connections: ConnectionCaps {
                total: defaults::MAX_CONNECTIONS,
                per_client: defaults::MAX_CONNECTIONS_PER_CLIENT,
            },
            header_read_timeout: defaults::HEADER_READ_TIMEOUT,
            max_header_count: defaults::MAX_HEADER_COUNT,
            max_header_bytes: defaults::MAX_HEADER_BYTES,
            send_timeout: defaults::SEND_TIMEOUT,
            tunnel_idle_timeout: Some(defaults::TUNNEL_IDLE_TIMEOUT),
        }
    }
}

/// Limits the pipeline checks on each request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLimits {
    /// A longer request target (path and query) is refused with 414.
    pub max_uri_bytes: usize,
    /// The body cap of a route that sets none; `None` is unlimited.
    pub max_body_bytes: Option<u64>,
    /// How long a request body may stall once the upstream is reading it.
    pub body_read_timeout: Duration,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_uri_bytes: defaults::MAX_URI_BYTES,
            max_body_bytes: None,
            body_read_timeout: defaults::BODY_READ_TIMEOUT,
        }
    }
}

/// What the pipeline settled for one request's body, carried in its extensions
/// to the forwarder: the most bytes it may have, and how long it may stall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyLimits {
    pub cap: Option<u64>,
    pub read_timeout: Duration,
}
