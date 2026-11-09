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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};
use hyper::body::Incoming;

type BodyError = Box<dyn Error + Send + Sync>;

struct Shared {
    body: Mutex<Incoming>,
    polled: AtomicBool,
}

/// Owns a request body across attempts.
pub struct BodyOwner {
    /// `None` when the request has no body at all, which is always replayable.
    shared: Option<Arc<Shared>>,
}

impl BodyOwner {
    pub fn new(body: Incoming) -> Self {
        if body.is_end_stream() {
            return Self { shared: None };
        }
        Self {
            shared: Some(Arc::new(Shared {
                body: Mutex::new(body),
                polled: AtomicBool::new(false),
            })),
        }
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
        Pin::new(&mut *body)
            .poll_frame(cx)
            .map_err(|err| Box::new(err) as BodyError)
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
                    let owner = BodyOwner::new(request.into_body());
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
        let (failed, is_connect, replayable) = with_incoming("POST", "hello", move |owner| {
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
        let echoed = with_incoming("POST", "hello world", move |owner| {
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
        let (is_connect, replayable) = with_incoming("POST", "payload", move |owner| {
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
        let replayable = with_incoming("GET", "", move |owner| {
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

    #[test]
    fn an_empty_attempt_body_reports_an_empty_stream() {
        let body = ReplayBody { shared: None };
        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
    }
}
