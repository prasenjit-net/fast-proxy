//! The HTTPS listener: accepts TCP connections, runs the TLS handshake under a
//! timeout and a bound on concurrent handshakes, and hands the encrypted stream
//! to the same connection handling as plain HTTP. Handshake outcomes are
//! counted by listener, result and version only.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_routing::SnapshotHolder;
use fast_proxy_telemetry::{HandshakeResult, Metrics};
use fast_proxy_tls::{
    build_server_config, ephemeral_unknown_host, ServerConfigError, ServerTlsPolicy,
};
use rustls::ServerConfig;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_rustls::server::TlsStream;
use tokio_rustls::LazyConfigAcceptor;

use crate::tls_resolver::{select_certificate, Selection, SnapshotCertResolver};
use crate::BoundListener;

/// The process-wide TLS settings of the data plane.
#[derive(Debug, Clone, Copy)]
pub struct TlsPlaneSettings {
    pub policy: ServerTlsPolicy,
    /// How long one handshake may take.
    pub handshake_timeout: Duration,
    /// How many handshakes may run at once; further connections are refused.
    pub max_concurrent_handshakes: usize,
    /// Fail the handshake for an unknown name instead of presenting a placeholder.
    pub reject_unknown_sni: bool,
}

/// Why the HTTPS listener could not be prepared.
#[derive(Debug, Error)]
pub enum TlsPlaneError {
    #[error(transparent)]
    Config(#[from] ServerConfigError),
    #[error("cannot create the placeholder certificate: {0}")]
    Placeholder(String),
}

/// The HTTPS listener with everything a handshake needs.
pub struct TlsPlane {
    pub(crate) listener: BoundListener,
    config: Arc<ServerConfig>,
    snapshots: Arc<SnapshotHolder>,
    settings: TlsPlaneSettings,
    handshakes: Arc<Semaphore>,
    metrics: Arc<Metrics>,
}

/// A completed handshake.
pub(crate) struct Accepted {
    pub stream: TlsStream<TcpStream>,
}

impl TlsPlane {
    /// Prepares the listener: the server configuration (policy, ALPN, tickets)
    /// with a resolver over the live snapshot, and the placeholder certificate.
    pub fn new(
        listener: BoundListener,
        snapshots: Arc<SnapshotHolder>,
        settings: TlsPlaneSettings,
        metrics: Arc<Metrics>,
    ) -> Result<Self, TlsPlaneError> {
        let placeholder = ephemeral_unknown_host(now_ms()).map_err(TlsPlaneError::Placeholder)?;
        let resolver = Arc::new(SnapshotCertResolver::new(
            snapshots.clone(),
            placeholder,
            settings.reject_unknown_sni,
        ));
        let config = Arc::new(build_server_config(&settings.policy, resolver)?);
        Ok(Self {
            listener,
            config,
            snapshots,
            handshakes: Arc::new(Semaphore::new(settings.max_concurrent_handshakes)),
            settings,
            metrics,
        })
    }

    /// The bound HTTPS listener (its name and address).
    pub fn listener(&self) -> &BoundListener {
        &self.listener
    }

    /// Runs the handshake on `stream`, or `None` when it failed, timed out, was
    /// refused because too many are running, or was rejected for its SNI. Every
    /// outcome is counted.
    pub(crate) async fn accept(&self, stream: TcpStream) -> Option<Accepted> {
        let name = self.listener.name.as_str();
        let Ok(_permit) = self.handshakes.clone().try_acquire_owned() else {
            self.metrics
                .tls_handshake(name, HandshakeResult::Failed, "-");
            return None;
        };
        let outcome =
            tokio::time::timeout(self.settings.handshake_timeout, self.handshake(stream)).await;
        match outcome {
            Err(_) => {
                self.metrics
                    .tls_handshake(name, HandshakeResult::Timeout, "-");
                None
            }
            Ok(Err(result)) => {
                self.metrics.tls_handshake(name, result, "-");
                None
            }
            Ok(Ok(stream)) => {
                let version = stream
                    .get_ref()
                    .1
                    .protocol_version()
                    .map_or_else(|| "-".to_string(), |version| format!("{version:?}"));
                self.metrics
                    .tls_handshake(name, HandshakeResult::Success, &version);
                Some(Accepted { stream })
            }
        }
    }

    async fn handshake(&self, stream: TcpStream) -> Result<TlsStream<TcpStream>, HandshakeResult> {
        let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream)
            .await
            .map_err(|_| HandshakeResult::Failed)?;
        let sni = start.client_hello().server_name().map(str::to_string);
        if matches!(
            select_certificate(
                &self.snapshots.load(),
                sni.as_deref(),
                self.settings.reject_unknown_sni
            ),
            Selection::Reject
        ) {
            return Err(HandshakeResult::RejectedSni);
        }
        start
            .into_stream(self.config.clone())
            .await
            .map_err(|_| HandshakeResult::Failed)
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
