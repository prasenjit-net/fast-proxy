//! The slow-client timeouts end to end (ADR 0029, spike G2): what hyper does
//! not bound itself. A connection that never sends a byte, a client that never
//! reads its response, a request body that stops arriving, and a tunnel that
//! carries nothing are each ended and counted, and the upstream is never blamed.

use std::time::Duration;

use fast_proxy_model::Config;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, MaybeTlsStream};

use crate::test_support::{eventually_within, get, route, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream, WsEchoServer};
use crate::{RequestLimits, ServerLimits};

const HOST: &str = "shop.example.com";
/// Generous: the watchdog ticks at a quarter of a rule, and CI machines are slow.
const LIMIT: Duration = Duration::from_secs(10);

async fn proxy(upstream: std::net::SocketAddr, limits: (ServerLimits, RequestLimits)) -> Harness {
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream, vec![route(1, "/")])],
    };
    Harness::proxying_with_limits(config, limits).await
}

fn metric(harness: &Harness, line_prefix: &str) -> Option<f64> {
    harness
        .metrics
        .render()
        .lines()
        .find(|line| line.starts_with(line_prefix))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

async fn connect(harness: &Harness) -> TcpStream {
    let stream = TcpStream::connect(harness.addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

/// Reads until the proxy closes the stream; `true` if it did within [`LIMIT`].
async fn closed_by_proxy(stream: &mut TcpStream) -> bool {
    let mut buf = vec![0u8; 64 * 1024];
    let read_all = async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    };
    tokio::time::timeout(LIMIT, read_all).await.is_ok()
}

fn ok_upstream() -> Behavior {
    Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    }
}

#[tokio::test]
async fn a_connection_that_never_sends_a_byte_is_closed_and_counted() {
    let upstream = TestUpstream::start(ok_upstream()).await;
    let limits = ServerLimits {
        header_read_timeout: Duration::from_millis(400),
        ..ServerLimits::default()
    };
    let proxy = proxy(upstream.addr, (limits, RequestLimits::default())).await;
    let mut silent = connect(&proxy).await;
    assert!(closed_by_proxy(&mut silent).await);
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_client_timeouts_total{kind="first_byte"}"#
        ),
        Some(1.0)
    );
    assert_eq!(
        get(proxy.addr, HOST, "/", "").await.status,
        200,
        "others still served"
    );
}

#[tokio::test]
async fn a_client_that_never_reads_its_response_is_disconnected() {
    // Large enough to fill the socket buffers both ways, so writes block.
    let body: &'static str = Box::leak("x".repeat(64 << 20).into_boxed_str());
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body,
    })
    .await;
    let limits = ServerLimits {
        send_timeout: Duration::from_millis(400),
        ..ServerLimits::default()
    };
    let proxy = proxy(upstream.addr, (limits, RequestLimits::default())).await;
    let mut reader = connect(&proxy).await;
    reader
        .write_all(format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(
        eventually_within(LIMIT, || {
            metric(&proxy, r#"fastproxy_client_timeouts_total{kind="send"}"#) == Some(1.0)
        })
        .await,
        "the stalled writer is noticed"
    );
    assert!(
        closed_by_proxy(&mut reader).await,
        "and the connection is closed"
    );
}

#[tokio::test]
async fn a_body_that_stops_arriving_is_408_and_the_upstream_is_not_blamed() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let limits = RequestLimits {
        body_read_timeout: Duration::from_millis(400),
        ..RequestLimits::default()
    };
    let proxy = proxy(upstream.addr, (ServerLimits::default(), limits)).await;
    let mut client = connect(&proxy).await;
    client
        .write_all(
            format!("POST / HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 100\r\n\r\nonly ten b")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut answer = vec![0u8; 1024];
    let read = tokio::time::timeout(LIMIT, client.read(&mut answer))
        .await
        .expect("the proxy answers")
        .unwrap();
    let answer = String::from_utf8_lossy(&answer[..read]);
    assert!(answer.starts_with("HTTP/1.1 408"), "{answer}");
    assert_eq!(
        metric(&proxy, r#"fastproxy_client_timeouts_total{kind="body"}"#),
        Some(1.0)
    );
    assert_eq!(metric(&proxy, "fastproxy_upstream_errors_total"), None);
}

#[tokio::test]
async fn an_idle_tunnel_is_closed_but_a_busy_one_is_not() {
    let server = WsEchoServer::start().await;
    let limits = ServerLimits {
        tunnel_idle_timeout: Some(Duration::from_millis(800)),
        ..ServerLimits::default()
    };
    let proxy = proxy(server.addr, (limits, RequestLimits::default())).await;
    let open = || async {
        let stream = connect(&proxy).await;
        let request = format!("ws://{HOST}/chat").into_client_request().unwrap();
        client_async(request, MaybeTlsStream::Plain(stream))
            .await
            .unwrap()
            .0
    };

    let mut busy = open().await;
    for n in 0..8 {
        busy.send(Message::text(format!("{n}"))).await.unwrap();
        assert_eq!(
            busy.next().await.unwrap().unwrap(),
            Message::text(format!("{n}"))
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    // Activity every 300 ms kept it open past the 800 ms limit; closed by its
    // client now, it is not a timeout.
    busy.close(None).await.unwrap();
    drop(busy);

    let mut idle = open().await;
    let ended = tokio::time::timeout(LIMIT, idle.next()).await;
    assert!(
        matches!(ended, Ok(None | Some(Err(_)))),
        "the idle tunnel was closed: {ended:?}"
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_client_timeouts_total{kind="tunnel_idle"}"#
        ),
        Some(1.0),
        "only the idle one"
    );
}
