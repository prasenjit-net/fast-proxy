//! Test-only PKI: an in-memory CA that issues leaf certificates with chosen
//! names and validity as PEM strings (tests upload them through the API, so no
//! files are involved), and a TLS client for handshake assertions.

mod client;
pub mod fixed_keys;
mod issue;

pub use client::{handshake, ClientHello, HandshakeError, Negotiated};
pub use issue::{Issued, LeafSpec, TestCa};
