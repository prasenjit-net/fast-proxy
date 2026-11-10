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
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_rustls::server::TlsStream;
use tokio_rustls::LazyConfigAcceptor;

use crate::connections::TlsConnection;
use crate::tls_resolver::{select_certificate, FixedCertificate, NoCertificate, Selection};
use crate::BoundListener;

/// Limits on HTTP/2 connections from clients (ADR 0009): bounded streams and
/// header lists, and a keep-alive ping so dead peers are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Http2Limits {
    /// Concurrent streams the client may open on one connection.
    pub max_concurrent_streams: u32,
    /// The largest request header list accepted, in bytes.
    pub max_header_list_size: u32,
    /// How often an idle connection is pinged.
    pub keep_alive_interval: Duration,
    /// How long a ping may go unanswered before the connection is closed.
    pub keep_alive_timeout: Duration,
}

impl Default for Http2Limits {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 128,
            max_header_list_size: 64 * 1024,
            keep_alive_interval: Duration::from_secs(30),
            keep_alive_timeout: Duration::from_secs(20),
        }
    }
}

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
    /// Limits applied when a client negotiates HTTP/2.
    pub http2: Http2Limits,
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
    placeholder: Arc<CertifiedKey>,
    snapshots: Arc<SnapshotHolder>,
    settings: TlsPlaneSettings,
    handshakes: Arc<Semaphore>,
    metrics: Arc<Metrics>,
}

/// A completed handshake.
pub(crate) struct Accepted {
    pub stream: TlsStream<TcpStream>,
    pub connection: TlsConnection,
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
        let config = Arc::new(build_server_config(
            &settings.policy,
            Arc::new(NoCertificate),
        )?);
        Ok(Self {
            listener,
            config,
            placeholder,
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
            Ok(Ok((stream, connection))) => {
                self.metrics
                    .tls_handshake(name, HandshakeResult::Success, &connection.version);
                Some(Accepted { stream, connection })
            }
        }
    }

    /// Reads the ClientHello, chooses the certificate once from the live
    /// snapshot, and finishes the handshake presenting exactly that one.
    async fn handshake(
        &self,
        stream: TcpStream,
    ) -> Result<(TlsStream<TcpStream>, TlsConnection), HandshakeResult> {
        let start = LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream)
            .await
            .map_err(|_| HandshakeResult::Failed)?;
        let sni = start.client_hello().server_name().map(str::to_string);
        let (key, certificate_id) = match select_certificate(
            &self.snapshots.load(),
            sni.as_deref(),
            self.settings.reject_unknown_sni,
        ) {
            Selection::Reject => return Err(HandshakeResult::RejectedSni),
            Selection::Placeholder => (Arc::clone(&self.placeholder), None),
            Selection::Subsystem { id, key } => (key, Some(id)),
        };
        let mut config = (*self.config).clone();
        config.cert_resolver = Arc::new(FixedCertificate(key));
        let stream = start
            .into_stream(Arc::new(config))
            .await
            .map_err(|_| HandshakeResult::Failed)?;
        let version = stream
            .get_ref()
            .1
            .protocol_version()
            .map_or_else(|| "-".to_string(), |version| format!("{version:?}"));
        Ok((
            stream,
            TlsConnection {
                sni,
                certificate_id,
                version,
            },
        ))
    }

    /// The HTTP/2 limits to apply to connections of this listener.
    pub(crate) fn http2_limits(&self) -> Http2Limits {
        self.settings.http2
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
