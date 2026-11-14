//! A request body that can be offered to more than one upstream attempt.
//!
//! `hyper::body::Incoming` cannot be cloned, and the pooled client takes the
//! body by value. [`BodyOwner`] keeps the incoming body behind a shared
//! pointer and hands each attempt a [`ReplayBody`] view of it. If an attempt
//! ends before the client ever polled the body (a connect failure), the next
//! attempt can still send the whole body; once a byte has been read the
//! request is no longer replayable. Spike G2 (ADR 0008) confirmed the client
//! does not poll a body before it has a connection.

use std::error::Error;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use hyper::body::Incoming;

type BodyError = Box<dyn Error + Send + Sync>;

/// What to learn about the request body while it streams to the upstream
/// (for the sampler): how many bytes were read, and optionally the first few.
#[derive(Debug, Clone, Copy)]
pub struct BodyObservation {
    /// How many bytes of the start of the body to keep; 0 only counts.
    pub capture_cap: usize,
}

/// What was seen of a request body. Shared between the attempts and whoever
/// reports the request afterwards; it holds no part of the connection, so
/// keeping it alive after the response costs nothing.
pub struct BodyStats {
    bytes: AtomicU64,
    tee: Option<Mutex<Tee>>,
}

struct Tee {
    cap: usize,
    captured: Vec<u8>,
}

impl BodyStats {
    fn new(observation: BodyObservation) -> Self {
        Self {
            bytes: AtomicU64::new(0),
            tee: (observation.capture_cap > 0).then(|| {
                Mutex::new(Tee {
                    cap: observation.capture_cap,
                    captured: Vec::new(),
                })
            }),
        }
    }

    /// Notes one frame of the body. The body is read once, by whichever attempt
    /// sent it, so nothing is ever noted twice.
    fn record(&self, data: &[u8]) {
        self.bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        let Some(tee) = &self.tee else {
            return;
        };
        let mut tee = tee.lock().unwrap_or_else(PoisonError::into_inner);
        let room = tee.cap.saturating_sub(tee.captured.len());
        if room > 0 {
            tee.captured
                .extend_from_slice(&data[..room.min(data.len())]);
        }
    }

    /// How many body bytes have been read so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    /// The captured start of the body, and whether the body was longer.
    pub fn captured(&self) -> Option<(Vec<u8>, bool)> {
        let tee = self.tee.as_ref()?;
        let tee = tee.lock().unwrap_or_else(PoisonError::into_inner);
        Some((tee.captured.clone(), self.bytes_read() > tee.cap as u64))
    }
}

struct Shared {
    body: Mutex<Incoming>,
    polled: AtomicBool,
    stats: Option<Arc<BodyStats>>,
}

/// Owns a request body across attempts.
pub struct BodyOwner {
    /// `None` when the request has no body at all, which is always replayable.
    shared: Option<Arc<Shared>>,
}

impl BodyOwner {
    /// `observation` asks to count (and perhaps capture) the body as it is read.
    pub fn new(body: Incoming, observation: Option<BodyObservation>) -> Self {
        if body.is_end_stream() {
            return Self { shared: None };
        }
        Self {
            shared: Some(Arc::new(Shared {
                body: Mutex::new(body),
                polled: AtomicBool::new(false),
                stats: observation.map(|observation| Arc::new(BodyStats::new(observation))),
            })),
        }
    }

    /// What was seen of the body; `None` unless it was observed (and it has
    /// one).
    pub fn stats(&self) -> Option<Arc<BodyStats>> {
        self.shared.as_ref().and_then(|shared| shared.stats.clone())
    }

    /// The body to send on the next attempt.
    pub fn attempt(&self) -> ReplayBody {
        ReplayBody {
            shared: self.shared.clone(),
        }
    }

    /// Whether another attempt can still send the complete body: there is
    /// none, or no attempt has read any of it.
    pub fn is_replayable(&self) -> bool {
        self.shared
            .as_ref()
            .is_none_or(|shared| !shared.polled.load(Ordering::SeqCst))
    }
}

/// One attempt's view of the request body.
pub struct ReplayBody {
    shared: Option<Arc<Shared>>,
}

impl Body for ReplayBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let Some(shared) = &self.shared else {
            return Poll::Ready(None);
        };
        shared.polled.store(true, Ordering::SeqCst);
        let mut body = shared.body.lock().unwrap_or_else(PoisonError::into_inner);
        let polled = Pin::new(&mut *body).poll_frame(cx);
        if let (Some(stats), Poll::Ready(Some(Ok(frame)))) = (&shared.stats, &polled) {
            if let Some(data) = frame.data_ref() {
                stats.record(data);
            }
        }
        polled.map_err(|err| Box::new(err) as BodyError)
    }

    fn is_end_stream(&self) -> bool {
        self.shared.as_ref().is_none_or(|shared| {
            let body = shared.body.lock().unwrap_or_else(PoisonError::into_inner);
            body.is_end_stream()
        })
    }

    fn size_hint(&self) -> SizeHint {
        match &self.shared {
            None => SizeHint::with_exact(0),
            Some(shared) => {
                let body = shared.body.lock().unwrap_or_else(PoisonError::into_inner);
                body.size_hint()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! These double as the findings of spike G2: they pin down how the pooled
    //! hyper client treats the request body, which retries depend on.

    use std::net::SocketAddr;

    use http::{Request, Response};
    use http_body_util::{BodyExt, Full};
    use hyper::service::service_fn;
    use hyper_util::client::legacy::connect::HttpConnector;
    use hyper_util::client::legacy::Client;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::mpsc;

    use super::*;

    /// Runs `scenario` inside a hyper server so it receives a real `Incoming`,
    /// then returns what it observed.
    async fn with_incoming<T: Send + 'static>(
        method: &str,
        body: &str,
        observation: Option<BodyObservation>,
        scenario: impl Fn(BodyOwner) -> std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>
            + Send
            + Sync
            + 'static,
    ) -> T {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let scenario = Arc::new(scenario);
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| {
                let (tx, scenario) = (tx.clone(), scenario.clone());
                async move {
                    let owner = BodyOwner::new(request.into_body(), observation);
                    let _ = tx.send(scenario(owner).await);
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::new())))
                }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let mut client = TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "{method} / HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        client.write_all(request.as_bytes()).await.unwrap();
        let outcome = rx.recv().await.unwrap();
        let mut sink = Vec::new();
        let _ = client.read_to_end(&mut sink).await;
        outcome
    }

    fn client() -> Client<HttpConnector, ReplayBody> {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(std::time::Duration::from_secs(2)));
        Client::builder(TokioExecutor::new())
            .retry_canceled_requests(false)
            .build(connector)
    }

    /// A port nothing listens on.
    async fn refused_addr() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap()
    }

    fn post_to(addr: SocketAddr, owner: &BodyOwner) -> Request<ReplayBody> {
        Request::post(format!("http://{addr}/"))
            .body(owner.attempt())
            .unwrap()
    }

    #[tokio::test]
    async fn a_connect_failure_never_polls_the_body_so_the_request_stays_replayable() {
        let refused = refused_addr().await;
        let (failed, is_connect, replayable) = with_incoming("POST", "hello", None, move |owner| {
            Box::pin(async move {
                let result = client().request(post_to(refused, &owner)).await;
                let err = result.unwrap_err();
                (true, err.is_connect(), owner.is_replayable())
            })
        })
        .await;
        assert!(
            failed && is_connect,
            "a refused connection is a connect error"
        );
        assert!(replayable, "the body was never polled");
    }

    #[tokio::test]
    async fn after_a_connect_failure_the_next_attempt_sends_the_whole_body() {
        let refused = refused_addr().await;
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = echo.accept().await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| async move {
                let bytes = request.into_body().collect().await.unwrap().to_bytes();
                Ok::<_, std::convert::Infallible>(Response::new(Full::new(bytes)))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let echoed = with_incoming("POST", "hello world", None, move |owner| {
            Box::pin(async move {
                assert!(client().request(post_to(refused, &owner)).await.is_err());
                assert!(owner.is_replayable());
                let response = client().request(post_to(echo_addr, &owner)).await.unwrap();
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                (
                    String::from_utf8(bytes.to_vec()).unwrap(),
                    owner.is_replayable(),
                )
            })
        })
        .await;
        assert_eq!(echoed.0, "hello world");
        assert!(!echoed.1, "once sent, the body has been consumed");
    }

    #[tokio::test]
    async fn a_peer_that_drops_the_connection_after_the_request_is_an_error_but_not_a_connect_error(
    ) {
        // Accepts, reads the request, then closes without answering.
        let rude = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rude_addr = rude.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = rude.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
            }
        });
        let (is_connect, replayable) = with_incoming("POST", "payload", None, move |owner| {
            Box::pin(async move {
                let err = client()
                    .request(post_to(rude_addr, &owner))
                    .await
                    .err()
                    .unwrap();
                (err.is_connect(), owner.is_replayable())
            })
        })
        .await;
        assert!(!is_connect, "the request reached the peer");
        assert!(
            !replayable,
            "the body was sent, so a POST must not be replayed"
        );
    }

    #[tokio::test]
    async fn a_request_without_a_body_is_always_replayable() {
        let rude = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let rude_addr = rude.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = rude.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 4096];
                let _ = stream.read(&mut buffer).await;
            }
        });
        let replayable = with_incoming("GET", "", None, move |owner| {
            Box::pin(async move {
                let request = Request::get(format!("http://{rude_addr}/"))
                    .body(owner.attempt())
                    .unwrap();
                assert!(client().request(request).await.is_err());
                owner.is_replayable()
            })
        })
        .await;
        assert!(replayable);
    }

    fn observed(cap: usize) -> Option<BodyObservation> {
        Some(BodyObservation { capture_cap: cap })
    }

    /// Reads the whole body through one attempt's view and returns what the
    /// observer saw.
    async fn read_through(owner: &BodyOwner) -> Option<Arc<BodyStats>> {
        let mut body = owner.attempt();
        while let Some(frame) = std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
        {
            frame.unwrap();
        }
        owner.stats()
    }

    #[tokio::test]
    async fn an_observed_body_is_counted_and_its_start_is_captured_up_to_the_cap() {
        for (cap, body, expect, truncated) in [
            (5usize, "hello world", "hello", true),
            (11, "hello world", "hello world", false),
            (12, "hello world", "hello world", false),
            (10, "hello world", "hello worl", true),
        ] {
            let (bytes, captured) = with_incoming("POST", body, observed(cap), move |owner| {
                Box::pin(async move {
                    let stats = read_through(&owner).await.unwrap();
                    (stats.bytes_read(), stats.captured().unwrap())
                })
            })
            .await;
            assert_eq!(bytes, body.len() as u64, "cap {cap}: every byte is counted");
            assert_eq!(captured.0, expect.as_bytes(), "cap {cap}");
            assert_eq!(captured.1, truncated, "cap {cap}");
        }
    }

    #[tokio::test]
    async fn a_cap_of_zero_only_counts() {
        let (bytes, captured) = with_incoming("POST", "hello world", observed(0), |owner| {
            Box::pin(async move {
                let stats = read_through(&owner).await.unwrap();
                (stats.bytes_read(), stats.captured())
            })
        })
        .await;
        assert_eq!(bytes, 11);
        assert!(captured.is_none(), "nothing is buffered when only counting");
    }

    #[tokio::test]
    async fn a_body_nobody_asked_to_observe_has_no_stats() {
        let stats = with_incoming("POST", "hello", None, |owner| {
            Box::pin(async move { read_through(&owner).await.is_none() })
        })
        .await;
        assert!(stats);
    }

    #[tokio::test]
    async fn a_retried_request_is_counted_and_captured_once() {
        let refused = refused_addr().await;
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = echo.accept().await.unwrap();
            let service = service_fn(move |request: Request<Incoming>| async move {
                let bytes = request.into_body().collect().await.unwrap().to_bytes();
                Ok::<_, std::convert::Infallible>(Response::new(Full::new(bytes)))
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
        let (bytes, captured) = with_incoming("POST", "hello world", observed(64), move |owner| {
            Box::pin(async move {
                assert!(client().request(post_to(refused, &owner)).await.is_err());
                let response = client().request(post_to(echo_addr, &owner)).await.unwrap();
                let _ = response.into_body().collect().await.unwrap();
                let stats = owner.stats().unwrap();
                (stats.bytes_read(), stats.captured().unwrap().0)
            })
        })
        .await;
        assert_eq!(
            bytes, 11,
            "the failed attempt never read the body, the second read it once"
        );
        assert_eq!(captured, b"hello world");
    }

    #[tokio::test]
    async fn the_stats_outlive_the_body() {
        let stats = with_incoming("POST", "hello", observed(8), |owner| {
            Box::pin(async move { read_through(&owner).await.unwrap() })
        })
        .await;
        assert_eq!(stats.bytes_read(), 5);
        assert_eq!(stats.captured().unwrap().0, b"hello");
    }

    #[test]
    fn an_empty_attempt_body_reports_an_empty_stream() {
        let body = ReplayBody { shared: None };
        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
    }
}
