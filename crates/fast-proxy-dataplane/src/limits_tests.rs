//! The process limits end to end (ADR 0029): connection caps when a connection
//! is accepted, the head limits hyper enforces, the target and body caps, and
//! framing that could smuggle a second request.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_limits::ConnectionCaps;
use fast_proxy_model::Config;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::test_support::{eventually, eventually_within, get, route, send_raw, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream};
use crate::{RequestLimits, ServerLimits};

const HOST: &str = "shop.example.com";

fn server(per_client: usize, total: usize) -> ServerLimits {
    ServerLimits {
        connections: ConnectionCaps { total, per_client },
        ..ServerLimits::default()
    }
}

async fn proxy(
    upstream: &TestUpstream,
    max_body_bytes: Option<u64>,
    limits: (ServerLimits, RequestLimits),
) -> Harness {
    let mut root = route(1, "/");
    root.max_body_bytes = max_body_bytes;
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![root])],
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

/// Whether the proxy closes `stream` (it reads end of stream, or a reset)
/// within a generous limit; a refused connection is slow to show on Windows.
async fn closed_by_proxy(stream: &mut TcpStream) -> bool {
    let mut buf = [0u8; 64];
    matches!(
        tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf)).await,
        Ok(Ok(0) | Err(_))
    )
}

async fn connect(harness: &Harness) -> TcpStream {
    let stream = TcpStream::connect(harness.addr).await.unwrap();
    stream.set_nodelay(true).unwrap();
    stream
}

// ---- connection caps ------------------------------------------------------

#[tokio::test]
async fn a_peer_over_its_cap_is_closed_while_its_open_connections_keep_working() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let proxy = proxy(&upstream, None, (server(2, 100), RequestLimits::default())).await;
    let mut first = connect(&proxy).await;
    let _second = connect(&proxy).await;
    assert!(eventually(|| proxy.plane.active_connections() == 2).await);

    let mut third = connect(&proxy).await;
    assert!(
        closed_by_proxy(&mut third).await,
        "the third is closed at once"
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_connections_refused_total{reason="max_connections_per_client"}"#
        ),
        Some(1.0)
    );

    first
        .write_all(
            format!("GET / HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut reply = Vec::new();
    first.read_to_end(&mut reply).await.unwrap();
    assert!(String::from_utf8_lossy(&reply).starts_with("HTTP/1.1 200"));
    assert!(eventually(|| proxy.plane.active_connections() == 1).await);
    assert_eq!(
        get(proxy.addr, HOST, "/", "").await.status,
        200,
        "a place came back"
    );
}

#[tokio::test]
async fn the_total_cap_holds_across_peers() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let proxy = proxy(&upstream, None, (server(0, 2), RequestLimits::default())).await;
    let held = [connect(&proxy).await, connect(&proxy).await];
    assert!(eventually(|| proxy.plane.active_connections() == 2).await);
    let mut over = connect(&proxy).await;
    assert!(closed_by_proxy(&mut over).await);
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_connections_refused_total{reason="max_connections"}"#
        ),
        Some(1.0)
    );
    drop(held);
    assert!(
        eventually_within(Duration::from_secs(10), || proxy.plane.active_connections()
            == 0)
        .await
    );
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
}

// ---- the request head -----------------------------------------------------

#[tokio::test]
async fn too_many_header_fields_or_too_large_a_head_is_431() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let limits = ServerLimits {
        max_header_count: 5,
        max_header_bytes: 8 * 1024,
        ..ServerLimits::default()
    };
    let proxy = proxy(&upstream, None, (limits, RequestLimits::default())).await;

    let many: String = (0..8).map(|n| format!("X-Extra-{n}: v\r\n")).collect();
    let reply = send_raw(
        proxy.addr,
        &format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n{many}Connection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(reply.status, 431);

    // hyper answers 431 and closes with the oversized head still unread, so the
    // kernel may reset the connection before the client reads the answer.
    let mut big = connect(&proxy).await;
    let head = format!(
        "GET / HTTP/1.1\r\nHost: {HOST}\r\nX-Big: {}\r\nConnection: close\r\n\r\n",
        "a".repeat(9 * 1024)
    );
    let _ = big.write_all(head.as_bytes()).await;
    let mut answer = Vec::new();
    match big.read_to_end(&mut answer).await {
        Ok(_) => assert!(
            String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 431"),
            "{}",
            String::from_utf8_lossy(&answer)
        ),
        Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset),
    }
    assert_eq!(upstream.request_count(), 0, "neither reached the upstream");
}

#[tokio::test]
async fn a_long_request_target_is_414_and_never_forwarded() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let limits = RequestLimits {
        max_uri_bytes: 64,
        max_body_bytes: None,
    };
    let proxy = proxy(&upstream, None, (ServerLimits::default(), limits)).await;
    let long = format!("/{}", "a".repeat(64));
    assert_eq!(get(proxy.addr, HOST, &long, "").await.status, 414);
    assert_eq!(get(proxy.addr, HOST, &long[..64], "").await.status, 200);
    assert_eq!(upstream.request_count(), 1);
}

// ---- the request body -----------------------------------------------------

#[tokio::test]
async fn a_declared_body_over_the_routes_cap_is_413_before_any_upstream_work() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let limits = RequestLimits {
        max_uri_bytes: 1024,
        max_body_bytes: Some(1_000_000),
    };
    // The route's own cap (10) wins over the process default.
    let proxy = proxy(&upstream, Some(10), (ServerLimits::default(), limits)).await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello world"
        ),
    )
    .await;
    assert_eq!(reply.status, 413);
    // Forwarding would connect before reading any body; give an accept that
    // was under way time to be counted, then check none happened.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(upstream.request_count(), 0);
    assert_eq!(
        upstream
            .connections
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "refused before an endpoint was even chosen"
    );

    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 10\r\nConnection: close\r\n\r\nhello worl"
        ),
    )
    .await;
    assert_eq!(reply.status, 200, "exactly the cap is fine");
}

#[tokio::test]
async fn a_streamed_body_over_its_cap_is_cut_with_413_and_the_upstream_is_not_blamed() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let limits = RequestLimits {
        max_uri_bytes: 1024,
        max_body_bytes: Some(16),
    };
    let proxy = Arc::new(proxy(&upstream, None, (ServerLimits::default(), limits)).await);
    let chunk = "a".repeat(10);
    let chunked = format!("a\r\n{chunk}\r\na\r\n{chunk}\r\na\r\n{chunk}\r\n0\r\n\r\n");
    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{chunked}"
        ),
    )
    .await;
    assert_eq!(reply.status, 413);
    assert!(
        upstream
            .body_bytes
            .load(std::sync::atomic::Ordering::SeqCst)
            <= 16,
        "no more than the cap was sent on"
    );
    assert_eq!(
        metric(&proxy, "fastproxy_upstream_errors_total"),
        None,
        "the endpoint is not blamed for the client's body"
    );
}

// ---- framing ----------------------------------------------------------------

#[tokio::test]
async fn length_and_chunked_together_is_forwarded_with_one_framing_only() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy(
        &upstream,
        None,
        (ServerLimits::default(), RequestLimits::default()),
    )
    .await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    let seen = upstream.last_headers();
    assert!(
        seen.get(http::header::CONTENT_LENGTH).is_none()
            || seen.get(http::header::TRANSFER_ENCODING).is_none(),
        "never both: {seen:?}"
    );
    assert_eq!(
        upstream
            .body_bytes
            .load(std::sync::atomic::Ordering::SeqCst),
        5
    );
}

#[tokio::test]
async fn a_transfer_encoding_that_is_not_chunked_is_refused() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy(
        &upstream,
        None,
        (ServerLimits::default(), RequestLimits::default()),
    )
    .await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST / HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: gzip\r\nConnection: close\r\n\r\nxx"
        ),
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(upstream.request_count(), 0);
}
