use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::connections::ConnectionTracker;
use crate::{BoundListener, Forward, RequestPipeline};

/// How long a client may take to send complete request headers.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for buffered request head data (and so for header size).
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// The running data plane: one accept loop per bound listener.
pub struct DataPlane {
    local_addrs: Vec<(String, SocketAddr)>,
    accept_loops: Vec<JoinHandle<()>>,
    connections: Arc<ConnectionTracker>,
}

impl DataPlane {
    /// Starts serving on the bound listeners. Cancelling `shutdown` stops
    /// accepting new connections; connections already open keep running.
    pub fn start<F: Forward>(
        listeners: Vec<BoundListener>,
        pipeline: Arc<RequestPipeline<F>>,
        shutdown: CancellationToken,
    ) -> Self {
        let connections = ConnectionTracker::new();
        let local_addrs = listeners
            .iter()
            .map(|bound| (bound.name.clone(), bound.local_addr()))
            .collect();
        let accept_loops = listeners
            .into_iter()
            .map(|bound| {
                tokio::spawn(accept_loop(
                    bound,
                    pipeline.clone(),
                    connections.clone(),
                    shutdown.clone(),
                ))
            })
            .collect();
        Self {
            local_addrs,
            accept_loops,
            connections,
        }
    }

    /// Each listener's name with the address it is actually bound to.
    pub fn local_addrs(&self) -> &[(String, SocketAddr)] {
        &self.local_addrs
    }

    /// Client connections currently open across all listeners.
    pub fn active_connections(&self) -> usize {
        self.connections.active()
    }

    /// Resolves once every accept loop has stopped (after shutdown).
    pub async fn stopped(self) {
        for accept_loop in self.accept_loops {
            // A panicked accept loop has already logged; nothing more to do.
            let _ = accept_loop.await;
        }
    }
}

async fn accept_loop<F: Forward>(
    bound: BoundListener,
    pipeline: Arc<RequestPipeline<F>>,
    connections: Arc<ConnectionTracker>,
    shutdown: CancellationToken,
) {
    loop {
        let accepted = tokio::select! {
            () = shutdown.cancelled() => return,
            accepted = bound.listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                let guard = connections.open();
                let pipeline = pipeline.clone();
                tokio::spawn(async move {
                    serve_connection(stream, peer, pipeline).await;
                    drop(guard);
                });
            }
            // Typically too many open files or an aborted handshake; back off
            // briefly so a persistent failure cannot spin the loop.
            Err(err) => {
                tracing::warn!(listener = %bound.name, "accept failed: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn serve_connection<F: Forward>(
    stream: TcpStream,
    peer: SocketAddr,
    pipeline: Arc<RequestPipeline<F>>,
) {
    let service = service_fn(move |request| {
        let pipeline = pipeline.clone();
        async move { Ok::<_, Infallible>(pipeline.handle(request, peer).await) }
    });
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEAD_BYTES)
        .keep_alive(true);
    // Upgrades stay enabled so WebSocket passthrough (D5) can build on this.
    let result = builder
        .http1_only()
        .serve_connection_with_upgrades(TokioIo::new(stream), service)
        .await;
    if let Err(err) = result {
        tracing::debug!(%peer, "connection ended with error: {err}");
    }
}
