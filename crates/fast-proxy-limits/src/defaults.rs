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
