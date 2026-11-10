use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::connections::{ConnectionContext, ConnectionTracker};
use crate::{BoundListener, Forward, Http2Limits, RequestPipeline, TlsPlane};

/// How long a client may take to send complete request headers.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound for buffered request head data (and so for header size).
const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Streams a client may reset before the server accepts them (hyper's default,
/// stated here because it is the rapid-reset mitigation).
const RAPID_RESET_PENDING_CAP: usize = 20;
/// Streams the server may reset because of protocol errors before the
/// connection is closed.
const RAPID_RESET_LOCAL_ERROR_CAP: usize = 1024;

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
        Self::start_with_tls(listeners, None, pipeline, shutdown)
    }

    /// Like [`DataPlane::start`], with an HTTPS listener beside the plain ones
    /// when TLS is on. The HTTPS listener is listed among
    /// [`DataPlane::local_addrs`] and drains like the others.
    pub fn start_with_tls<F: Forward>(
        listeners: Vec<BoundListener>,
        tls: Option<TlsPlane>,
        pipeline: Arc<RequestPipeline<F>>,
        shutdown: CancellationToken,
    ) -> Self {
        let connections = ConnectionTracker::new(pipeline.metrics());
        let force_close = CancellationToken::new();
        let signals = Signals {
            shutdown,
            force_close: force_close.clone(),
        };
        let mut local_addrs: Vec<(String, SocketAddr)> = listeners
            .iter()
            .map(|bound| (bound.name.clone(), bound.local_addr()))
            .collect();
        let mut accept_loops: Vec<JoinHandle<()>> = listeners
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
        if let Some(tls) = tls {
            local_addrs.push((tls.listener.name.clone(), tls.listener.local_addr()));
            accept_loops.push(tokio::spawn(tls_accept_loop(
                Arc::new(tls),
                pipeline,
                connections.clone(),
                signals,
            )));
        }
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
                    scheme: "http",
                    tls: None,
                };
                tokio::spawn(serve_connection(
                    stream,
                    peer,
                    pipeline.clone(),
                    context,
                    signals.shutdown.clone(),
                    None,
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

/// Accepts TCP connections on the HTTPS listener and finishes each handshake
/// in its own task, so a slow handshake never delays the accept loop.
async fn tls_accept_loop<F: Forward>(
    tls: Arc<TlsPlane>,
    pipeline: Arc<RequestPipeline<F>>,
    connections: Arc<ConnectionTracker>,
    signals: Signals,
) {
    loop {
        let accepted = tokio::select! {
            () = signals.shutdown.cancelled() => return,
            accepted = tls.listener.listener.accept() => accepted,
        };
        match accepted {
            Ok((stream, peer)) => {
                let (tls, pipeline) = (tls.clone(), pipeline.clone());
                let context = ConnectionContext {
                    guard: Arc::new(connections.open()),
                    force_close: signals.force_close.clone(),
                    scheme: "https",
                    tls: None,
                };
                let shutdown = signals.shutdown.clone();
                tokio::spawn(async move {
                    let force_close = context.force_close.clone();
                    let handshake = tls.accept(stream);
                    let accepted = tokio::select! {
                        accepted = handshake => accepted,
                        () = shutdown.cancelled() => None,
                        () = force_close.cancelled() => None,
                    };
                    if let Some(accepted) = accepted {
                        let context = ConnectionContext {
                            tls: Some(Arc::new(accepted.connection)),
                            ..context
                        };
                        let limits = tls.http2_limits();
                        serve_connection(
                            accepted.stream,
                            peer,
                            pipeline,
                            context,
                            shutdown,
                            Some(limits),
                        )
                        .await;
                    }
                });
            }
            Err(err) => {
                tracing::warn!(listener = %tls.listener.name, "accept failed: {err}");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// Applies the HTTP/2 limits: bounded streams and header lists, a keep-alive
/// ping, and the h2 rapid-reset mitigations (a cap on streams a client resets
/// before the server accepted them, and on streams reset because of errors).
fn configure_http2(builder: &mut auto::Builder<TokioExecutor>, limits: Http2Limits) {
    builder
        .http2()
        .timer(TokioTimer::new())
        .max_concurrent_streams(limits.max_concurrent_streams)
        .max_header_list_size(limits.max_header_list_size)
        .keep_alive_interval(limits.keep_alive_interval)
        .keep_alive_timeout(limits.keep_alive_timeout)
        .max_pending_accept_reset_streams(RAPID_RESET_PENDING_CAP)
        .max_local_error_reset_streams(RAPID_RESET_LOCAL_ERROR_CAP);
}

async fn serve_connection<F: Forward, S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    peer: SocketAddr,
    pipeline: Arc<RequestPipeline<F>>,
    context: ConnectionContext,
    shutdown: CancellationToken,
    http2: Option<Http2Limits>,
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
    // HTTP/2 only where the listener offers it (TLS, negotiated by ALPN); the
    // connection then speaks whichever protocol the client chose.
    let builder = match http2 {
        Some(limits) => {
            configure_http2(&mut builder, limits);
            builder
        }
        None => builder.http1_only(),
    };
    // Upgrades are enabled so a protocol switch (WebSocket) can take over the socket.
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
