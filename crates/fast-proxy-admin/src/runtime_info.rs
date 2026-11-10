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
}

/// Facts about the running process that only the binary knows, reported by
/// `GET /api/status`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeInfo {
    pub listeners: Vec<ListenerInfo>,
    pub database_path: String,
    pub tls: TlsInfo,
}
