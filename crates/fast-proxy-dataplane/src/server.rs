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

use crate::connections::{ConnectionContext, ConnectionTracker};
use crate::{BoundListener, Forward, RequestPipeline};

/// How long a client may take to send complete request headers.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for buffered request head data (and so for header size).
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// What a drain left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrainReport {
    /// Connections (including upgraded tunnels) still open when the grace
    /// period ended, which were closed forcibly.
    pub forced_connections: usize,
}

/// Cancellation signals every connection watches.
#[derive(Clone)]
struct Signals {
    /// Stop accepting; finish in-flight requests, then close.
    shutdown: CancellationToken,
    /// The grace period is over: close right now.
    force_close: CancellationToken,
}

/// The running data plane: one accept loop per bound listener.
pub struct DataPlane {
    local_addrs: Vec<(String, SocketAddr)>,
    accept_loops: Vec<JoinHandle<()>>,
    connections: Arc<ConnectionTracker>,
    force_close: CancellationToken,
}

impl DataPlane {
    /// Starts serving on the bound listeners. Cancelling `shutdown` begins a
    /// graceful stop: no new connections are accepted, idle ones close, and
    /// in-flight requests may finish. Await [`DataPlane::stopped`] to drain.
    pub fn start<F: Forward>(
        listeners: Vec<BoundListener>,
        pipeline: Arc<RequestPipeline<F>>,
        shutdown: CancellationToken,
    ) -> Self {
        let connections = ConnectionTracker::new();
        let force_close = CancellationToken::new();
        let signals = Signals {
            shutdown,
            force_close: force_close.clone(),
        };
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
                    signals.clone(),
                ))
            })
            .collect();
        Self {
            local_addrs,
            accept_loops,
            connections,
            force_close,
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

    /// Waits for the accept loops to stop (after shutdown was signalled),
    /// then for open connections to finish, for at most `grace`. Anything
    /// still open after that is closed forcibly.
    pub async fn stopped(self, grace: Duration) -> DrainReport {
        for accept_loop in self.accept_loops {
            // A panicked accept loop has already logged; nothing more to do.
            let _ = accept_loop.await;
        }
        let drained = tokio::time::timeout(grace, self.connections.wait_idle()).await;
        if drained.is_ok() {
            return DrainReport {
                forced_connections: 0,
            };
        }
        let forced_connections = self.connections.active();
        self.force_close.cancel();
        // Closing is prompt; wait so the caller knows nothing is left running.
        self.connections.wait_idle().await;
        DrainReport { forced_connections }
    }
}

async fn accept_loop<F: Forward>(
    bound: BoundListener,
    pipeline: Arc<RequestPipeline<F>>,
    connections: Arc<ConnectionTracker>,
    signals: Signals,
) {
    loop {
        let accepted = tokio::select! {
            () = signals.shutdown.cancelled() => return,
            accepted = bound.listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                let context = ConnectionContext {
                    guard: Arc::new(connections.open()),
                    force_close: signals.force_close.clone(),
                };
                tokio::spawn(serve_connection(
                    stream,
                    peer,
                    pipeline.clone(),
                    context,
                    signals.shutdown.clone(),
                ));
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
    context: ConnectionContext,
    shutdown: CancellationToken,
) {
    let force_close = context.force_close.clone();
    let service = service_fn(move |mut request| {
        let pipeline = pipeline.clone();
        request.extensions_mut().insert(context.clone());
        async move { Ok::<_, Infallible>(pipeline.handle(request, peer).await) }
    });
    let mut builder = auto::Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT)
        .max_buf_size(MAX_HEAD_BYTES)
        .keep_alive(true);
    // Upgrades are enabled so a protocol switch (WebSocket) can take over the socket.
    let builder = builder.http1_only();
    let connection = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
    tokio::pin!(connection);

    let result = tokio::select! {
        result = connection.as_mut() => result,
        () = shutdown.cancelled() => {
            // Finish the request in flight (idle connections close at once),
            // but never past the drain deadline.
            connection.as_mut().graceful_shutdown();
            tokio::select! {
                result = connection.as_mut() => result,
                () = force_close.cancelled() => return,
            }
        }
    };
    if let Err(err) = result {
        tracing::debug!(%peer, "connection ended with error: {err}");
    }
}
