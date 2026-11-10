//! An in-process upstream server with configurable behavior, recording what
//! it receives.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, Request, Response};
use http_body_util::channel::{Channel, Sender};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::ResponseBody;

/// What the upstream does with each request.
#[derive(Clone)]
pub enum Behavior {
    /// Respond immediately with this status, headers, and body.
    Respond {
        status: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: &'static str,
    },
    /// Wait this long, then respond 200.
    Sleep(Duration),
    /// Never respond; signal `dropped` when the request is abandoned.
    HoldUntilDropped,
    /// Read the whole request body, counting bytes, then respond 200.
    CountBody,
    /// Respond with a body fed chunk by chunk through `stream_sender`.
    StreamResponse,
}

/// A request as the upstream saw it.
pub struct Recorded {
    pub method: String,
    pub path_and_query: String,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub struct TestUpstream {
    pub addr: SocketAddr,
    pub connections: AtomicUsize,
    pub body_bytes: AtomicUsize,
    pub requests: Mutex<Vec<Recorded>>,
    /// Signalled when the first body chunk of a `CountBody` request arrives.
    pub first_chunk: Notify,
    /// Signalled when a `HoldUntilDropped` request is abandoned.
    pub dropped: Notify,
    /// The sender feeding the current `StreamResponse` body.
    pub stream_sender: Mutex<Option<Sender<Bytes>>>,
    pub stream_started: Notify,
    /// When non-zero, every request is answered with this status instead of
    /// following `behavior` (see [`TestUpstream::fail_with`]).
    forced_status: AtomicU16,
    /// Stops the current accept loop and drops its open connections.
    stop: Mutex<CancellationToken>,
    behavior: Behavior,
}

impl TestUpstream {
    pub async fn start(behavior: Behavior) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = Arc::new(Self {
            addr: listener.local_addr().unwrap(),
            connections: AtomicUsize::new(0),
            body_bytes: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            first_chunk: Notify::new(),
            dropped: Notify::new(),
            stream_sender: Mutex::new(None),
            stream_started: Notify::new(),
            forced_status: AtomicU16::new(0),
            stop: Mutex::new(CancellationToken::new()),
            behavior,
        });
        upstream.accept_on(listener);
        upstream
    }

    /// Serves connections from `listener` until [`TestUpstream::kill`].
    fn accept_on(self: &Arc<Self>, listener: TcpListener) {
        let state = self.clone();
        let stop = CancellationToken::new();
        *self.stop.lock().unwrap() = stop.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    () = stop.cancelled() => return,
                };
                let Ok((stream, _)) = accepted else {
                    return;
                };
                state.connections.fetch_add(1, Ordering::SeqCst);
                let (state, stop) = (state.clone(), stop.clone());
                tokio::spawn(async move {
                    let service = service_fn(move |request| handle(state.clone(), request));
                    let serving = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service);
                    tokio::select! {
                        _ = serving => {}
                        () = stop.cancelled() => {}
                    }
                });
            }
        });
    }

    /// Simulates a crashed backend: stops listening and drops every open
    /// connection, so new connects are refused.
    pub fn kill(&self) {
        self.stop.lock().unwrap().cancel();
    }

    /// Brings a killed backend back on the same address.
    pub async fn revive(self: &Arc<Self>) {
        let mut bound = None;
        for _ in 0..100 {
            match TcpListener::bind(self.addr).await {
                Ok(listener) => {
                    bound = Some(listener);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
        self.accept_on(bound.expect("the killed backend's port should become free"));
    }

    /// Answers every request with `status` (for example 503) until
    /// [`TestUpstream::recover`].
    pub fn fail_with(&self, status: u16) {
        self.forced_status.store(status, Ordering::SeqCst);
    }

    /// Goes back to the configured behavior.
    pub fn recover(&self) {
        self.forced_status.store(0, Ordering::SeqCst);
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// The most recent recorded request's head.
    pub fn last_headers(&self) -> HeaderMap {
        self.requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .headers
            .clone()
    }

    pub fn last_path(&self) -> String {
        self.requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .path_and_query
            .clone()
    }
}

fn boxed(body: &'static str) -> ResponseBody {
    Full::new(Bytes::from(body))
        .map_err(|never| match never {})
        .boxed()
}

/// Notifies `dropped` when the request future is abandoned mid-flight.
struct DropSignal(Arc<TestUpstream>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        self.0.dropped.notify_one();
    }
}

async fn handle(
    state: Arc<TestUpstream>,
    request: Request<Incoming>,
) -> Result<Response<ResponseBody>, std::convert::Infallible> {
    let (parts, mut body) = request.into_parts();
    let mut recorded = Recorded {
        method: parts.method.to_string(),
        path_and_query: parts
            .uri
            .path_and_query()
            .map_or_else(String::new, ToString::to_string),
        headers: parts.headers,
        body: Vec::new(),
    };
    let keep_body = matches!(
        state.behavior,
        Behavior::Respond { .. } | Behavior::Sleep(_) | Behavior::StreamResponse
    );

    match state.behavior.clone() {
        Behavior::CountBody => {
            let mut first = true;
            while let Some(Ok(frame)) = body.frame().await {
                if let Ok(data) = frame.into_data() {
                    state.body_bytes.fetch_add(data.len(), Ordering::SeqCst);
                    if first {
                        first = false;
                        state.first_chunk.notify_one();
                    }
                }
            }
        }
        _ if keep_body => {
            while let Some(Ok(frame)) = body.frame().await {
                if let Ok(data) = frame.into_data() {
                    recorded.body.extend_from_slice(&data);
                }
            }
        }
        _ => {}
    }
    state.requests.lock().unwrap().push(recorded);

    let forced = state.forced_status.load(Ordering::SeqCst);
    if forced != 0 {
        let mut response = Response::new(boxed("forced"));
        *response.status_mut() = http::StatusCode::from_u16(forced).unwrap();
        return Ok(response);
    }

    Ok(match state.behavior.clone() {
        Behavior::Respond {
            status,
            headers,
            body,
        } => {
            let mut response = Response::new(boxed(body));
            *response.status_mut() = http::StatusCode::from_u16(status).unwrap();
            for (name, value) in headers {
                response.headers_mut().append(
                    http::HeaderName::from_static(name),
                    http::HeaderValue::from_static(value),
                );
            }
            response
        }
        Behavior::Sleep(duration) => {
            tokio::time::sleep(duration).await;
            Response::new(boxed("slow"))
        }
        Behavior::CountBody => Response::new(boxed("counted")),
        Behavior::HoldUntilDropped => {
            let _signal = DropSignal(state.clone());
            tokio::time::sleep(Duration::from_secs(30)).await;
            Response::new(boxed("late"))
        }
        Behavior::StreamResponse => {
            let (sender, channel) = Channel::<Bytes>::new(8);
            *state.stream_sender.lock().unwrap() = Some(sender);
            state.stream_started.notify_one();
            Response::new(channel.map_err(|never| match never {}).boxed())
        }
    })
}

/// A real WebSocket server that echoes every text and binary message, and
/// records the headers of each handshake it saw.
pub struct WsEchoServer {
    pub addr: SocketAddr,
    pub handshakes: Arc<Mutex<Vec<HeaderMap>>>,
}

impl WsEchoServer {
    // The handshake callback's error type is dictated by tokio-tungstenite.
    #[allow(clippy::result_large_err)]
    pub async fn start() -> Self {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
        use tokio_tungstenite::tungstenite::Message;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handshakes = Arc::new(Mutex::new(Vec::new()));
        let recorded = handshakes.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let callback = |request: &Request, response: Response| {
                        recorded.lock().unwrap().push(request.headers().clone());
                        Ok(response)
                    };
                    let Ok(mut socket) =
                        tokio_tungstenite::accept_hdr_async(stream, callback).await
                    else {
                        return;
                    };
                    while let Some(Ok(message)) = socket.next().await {
                        if matches!(message, Message::Text(_) | Message::Binary(_))
                            && socket.send(message).await.is_err()
                        {
                            return;
                        }
                    }
                });
            }
        });
        Self { addr, handshakes }
    }
}
