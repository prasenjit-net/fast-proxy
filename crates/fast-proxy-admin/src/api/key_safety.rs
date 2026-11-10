//! Requests that carry a private key are accepted only from a loopback peer
//! until the admin plane has authentication (M5, ADR 0010).

use std::net::SocketAddr;

use crate::{AppError, AppResult};

/// Refuses a request that carries a private key unless its peer is loopback.
pub fn require_loopback_for_keys(peer: SocketAddr) -> AppResult<()> {
    if peer.ip().is_loopback() {
        return Ok(());
    }
    Err(AppError::Forbidden(
        "private keys are accepted only from the machine the proxy runs on (loopback) until \
         the admin plane has authentication; run the request there"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_peers_may_send_keys() {
        for peer in ["127.0.0.1:1", "127.8.9.1:1", "[::1]:1"] {
            assert!(
                require_loopback_for_keys(peer.parse().unwrap()).is_ok(),
                "{peer}"
            );
        }
        for peer in [
            "192.168.1.5:1",
            "10.0.0.1:1",
            "[2001:db8::1]:1",
            "203.0.113.9:1",
        ] {
            let err = require_loopback_for_keys(peer.parse().unwrap()).unwrap_err();
            assert!(matches!(err, AppError::Forbidden(_)), "{peer}");
        }
    }
}
