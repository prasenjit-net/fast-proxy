use std::net::SocketAddr;

/// One data plane listener as actually bound.
#[derive(Debug, Clone)]
pub struct ListenerInfo {
    pub name: String,
    pub addr: SocketAddr,
}

/// The process-wide TLS settings as running.
#[derive(Debug, Clone, Default)]
pub struct TlsInfo {
    /// TLS is on: the HTTPS listener runs.
    pub enabled: bool,
    /// The HTTPS listener's bound address, when it runs.
    pub listen: Option<SocketAddr>,
    /// Whether HTTP/2 is offered to clients.
    pub http2: bool,
    /// The lowest TLS version accepted: `1.2` or `1.3`.
    pub min_version: String,
    /// The port a redirect to HTTPS names (443 is left out of the URL).
    pub redirect_port: u16,
}

/// The process limits in force (`[limits]` in `config.toml`, ADR 0029), as
/// `GET /api/limits` reports them. Times are milliseconds; `None` is off or
/// unlimited.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LimitsInfo {
    pub max_connections: usize,
    /// 0 means the per-client check is off.
    pub max_connections_per_client: usize,
    pub header_read_timeout_ms: u64,
    pub body_read_timeout_ms: u64,
    pub send_timeout_ms: u64,
    pub tunnel_idle_timeout_ms: Option<u64>,
    pub max_header_count: usize,
    pub max_header_bytes: usize,
    pub max_uri_bytes: usize,
    pub max_body_bytes: Option<u64>,
}

/// Facts about the running process that only the binary knows, reported by
/// `GET /api/status`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeInfo {
    pub listeners: Vec<ListenerInfo>,
    pub database_path: String,
    pub tls: TlsInfo,
    /// Request sampling runs in this process (`[sampling].enabled`); the
    /// samples of other instances can be read either way.
    pub sampling_enabled: bool,
    /// The process limits, for `GET /api/limits`.
    pub limits: LimitsInfo,
}
