//! Test support only: an in-process ACME server (RFC 8555, the subset the
//! proxy's client uses) over HTTPS on loopback.
//!
//! It is honest where it matters to the proxy: an HTTP-01 challenge is
//! validated by **really fetching** `/.well-known/acme-challenge/<token>` from
//! the targets the test names (round robin, so a cluster test can make every
//! instance answer), and finalize signs the client's own CSR with an in-memory
//! CA. It trusts the client's JWS signatures (the opt-in Pebble job checks real
//! signing) and keeps everything in memory.
//!
//! Never use it outside `dev-dependencies` and tests.

mod protocol;
mod server;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fast_proxy_test_pki::TestCa;

pub use state::Validation;

use crate::state::State;

/// A running test CA. Dropping it stops the server.
pub struct TestAcme {
    /// The directory URL to configure (`https://127.0.0.1:<port>/dir`).
    pub directory: String,
    /// A PEM file with the root of the server's TLS certificate (`directory_ca`).
    pub directory_ca: PathBuf,
    /// The CA that signs issued certificates; a client trusting it trusts them.
    pub issuer_pem: String,
    state: Arc<Mutex<State>>,
    _server: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for TestAcme {
    fn drop(&mut self) {
        self._server.abort();
    }
}

impl TestAcme {
    /// Starts a server on an ephemeral loopback port.
    pub async fn start() -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let tls_ca = TestCa::new("test acme tls");
        let directory_ca = dir.path().join("acme-root.pem");
        std::fs::write(&directory_ca, &tls_ca.pem).expect("writing the root");
        let issuer = TestCa::new("test acme issuer");
        let issuer_pem = issuer.pem.clone();
        let (addr, server, state) = server::start(&tls_ca, issuer).await;
        Self {
            directory: format!("https://{addr}/dir"),
            directory_ca,
            issuer_pem,
            state,
            _server: server,
            _dir: dir,
        }
    }

    fn with_state<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    /// Where challenges are fetched from, in turn (a proxy's plain listeners).
    pub fn set_challenge_targets(&self, targets: Vec<SocketAddr>) {
        self.with_state(|state| state.targets = targets);
    }

    /// Answers every new order with `429 rateLimited` while on.
    pub fn set_rate_limited(&self, on: bool) {
        self.with_state(|state| state.rate_limited = on);
    }

    /// Issued certificates last this many days (default 90).
    pub fn set_lifetime_days(&self, days: i64) {
        self.with_state(|state| state.lifetime_days = days);
    }

    /// How many orders were created.
    pub fn orders(&self) -> usize {
        self.with_state(|state| state.orders.len())
    }

    /// How many accounts were registered.
    pub fn accounts(&self) -> usize {
        self.with_state(|state| state.accounts.len())
    }

    /// How many certificates were issued.
    pub fn issued(&self) -> usize {
        self.with_state(|state| state.certificates.len())
    }

    /// Every challenge fetch so far, in order.
    pub fn validations(&self) -> Vec<Validation> {
        self.with_state(|state| state.validations.clone())
    }
}
