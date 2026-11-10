//! Serving the admin router over TLS (ADR 0016). The certificate is a stored
//! one chosen by name and read from the live cache on every handshake, so an
//! uploaded or renewed certificate is served by the next connection with no
//! restart. When the admin plane serves TLS the plain listener is never opened.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::ConnectInfo;
use axum::Router;
use fast_proxy_config_service::ConfigService;
use hyper::body::Incoming;
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// How long one handshake may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Handshakes running at once; the admin plane is not a busy place.
const MAX_CONCURRENT_HANDSHAKES: usize = 64;
/// How long open connections get to finish once shutdown starts.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Presents the stored certificate called `name`, looked up on every handshake.
pub struct StoredCertificate {
    config: Arc<ConfigService>,
    name: String,
}

impl StoredCertificate {
    pub fn new(config: Arc<ConfigService>, name: impl Into<String>) -> Self {
        Self {
            config,
            name: name.into(),
        }
    }
}

impl std::fmt::Debug for StoredCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCertificate")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for StoredCertificate {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.config
            .certificate_by_name(&self.name)
            .map(|handle| handle.key.clone())
    }
}

/// Accepts TLS connections on `listener` and serves `router` on them (HTTP/1.1
/// and, when the configuration offers it, HTTP/2, with upgrades so the
/// WebSocket works) until `shutdown` is cancelled, then lets open connections
/// finish within a grace period. Each request carries its peer address as
/// `ConnectInfo`, exactly as on the plain listener.
pub async fn serve_tls(
    listener: TcpListener,
    router: Router,
    tls: Arc<ServerConfig>,
    shutdown: CancellationToken,
) {
    let acceptor = TlsAcceptor::from(tls);
    let handshakes = Arc::new(Semaphore::new(MAX_CONCURRENT_HANDSHAKES));
    let graceful = GracefulShutdown::new();
    loop {
        let accepted = tokio::select! {
            () = shutdown.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        let (tcp, peer) = match accepted {
            Ok(connection) => connection,
            Err(err) => {
                tracing::warn!("the admin listener could not accept a connection: {err}");
                continue;
            }
        };
        let Ok(permit) = handshakes.clone().try_acquire_owned() else {
            continue;
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        let watcher = graceful.watcher();
        tokio::spawn(async move {
            let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp)).await;
            drop(permit);
            let Ok(Ok(stream)) = handshake else {
                return;
            };
            let service = TowerToHyperService::new(router.map_request(with_peer(peer)));
            let builder = Builder::new(TokioExecutor::new());
            let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
            let _ = watcher.watch(connection).await;
        });
    }
    if tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("closing admin connections still open after {SHUTDOWN_GRACE:?}");
    }
}

/// Adds the peer's address to a request, as `into_make_service_with_connect_info`
/// does on the plain listener.
fn with_peer(peer: SocketAddr) -> impl Fn(Request<Incoming>) -> Request<Incoming> + Clone {
    move |mut request| {
        request.extensions_mut().insert(ConnectInfo(peer));
        request
    }
}
