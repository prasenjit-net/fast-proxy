//! The default of every process limit (ADR 0029), in one table: the config file,
//! the data plane and the docs all read them from here.

use std::time::Duration;

/// Open client connections across all listeners, upgraded tunnels included.
pub const MAX_CONNECTIONS: usize = 10_000;
/// Open connections from one peer address; 0 turns the check off.
pub const MAX_CONNECTIONS_PER_CLIENT: usize = 256;
/// How long a client may take to send a request head (also the longest a
/// keep-alive connection may sit idle between requests: hyper times both).
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Request header fields; more are refused with 431.
pub const MAX_HEADER_COUNT: usize = 100;
/// Bytes of request head hyper buffers; a larger head is refused with 431.
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Bytes of request target (path and query); longer is refused with 414.
pub const MAX_URI_BYTES: usize = 8 * 1024;
/// How long a request body may stall (no bytes arrive) once the upstream is
/// reading it; the request is then answered 408 (ADR 0029).
pub const BODY_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long writes to a client may stay blocked (it is not reading) before the
/// connection is closed.
pub const SEND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long an upgraded tunnel (WebSocket) may carry no bytes either way.
pub const TUNNEL_IDLE_TIMEOUT: Duration = Duration::from_secs(3600);
