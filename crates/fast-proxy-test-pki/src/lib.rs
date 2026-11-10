//! Test-only PKI: an in-memory CA that issues leaf certificates with chosen
//! names and validity as PEM strings (tests upload them through the API, so no
//! files are involved), and a TLS client for handshake assertions.

mod client;
mod issue;
mod key_formats;

pub use client::{handshake, request, ClientHello, HandshakeError, Negotiated};
pub use issue::{Issued, LeafSpec, TestCa};
pub use key_formats::pkcs8_pem_to_sec1_pem;
