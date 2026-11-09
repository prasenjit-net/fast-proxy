//! An in-process upstream server with configurable behavior, recording what
//! it receives.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
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
            behavior,
        });
        let state = upstream.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                state.connections.fetch_add(1, Ordering::SeqCst);
                let state = state.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |request| handle(state.clone(), request));
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        upstream
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
