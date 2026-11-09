use std::net::SocketAddr;

/// One data plane listener as actually bound.
#[derive(Debug, Clone)]
pub struct ListenerInfo {
    pub name: String,
    pub addr: SocketAddr,
}

/// Facts about the running process that only the binary knows, reported by
/// `GET /api/status`.
#[derive(Debug, Clone, Default)]
pub struct RuntimeInfo {
    pub listeners: Vec<ListenerInfo>,
    pub database_path: String,
}
