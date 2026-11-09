//! End-to-end forwarding tests: the real proxy with the real upstream client
//! against in-process upstreams. They also settle the P1 questions about the
//! pooled hyper client: connection reuse, per-request timeouts, body
//! streaming without buffering, and cancellation when the client leaves.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use fast_proxy_model::{Config, RouteConfig, UpstreamPrefix};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::test_support::{eventually, get, parse_reply, route, send_raw, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const HOST: &str = "shop.example.com";
const OK: Behavior = Behavior::Respond {
    status: 200,
    headers: Vec::new(),
    body: "ok",
};

async fn proxy_to(upstream: &TestUpstream, routes: Vec<RouteConfig>) -> Harness {
    proxy_with(upstream, routes, &[]).await
}

async fn proxy_with(
    upstream: &TestUpstream,
    routes: Vec<RouteConfig>,
    trusted: &[&str],
) -> Harness {
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, routes)],
    };
    Harness::proxying(config, trusted).await
}

fn value(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(|v| v.to_str().unwrap().to_string())
}

// ---- basic forwarding ---------------------------------------------------

#[tokio::test]
async fn forwards_a_get_and_returns_the_upstream_response() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 201,
        headers: vec![("x-custom", "yes")],
        body: "created",
    })
    .await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let reply = get(proxy.addr, HOST, "/things?a=1", "").await;
    assert_eq!(reply.status, 201);
    assert_eq!(reply.body, "created");
    assert_eq!(reply.header("x-custom"), Some("yes"));
    assert_eq!(upstream.last_path(), "/things?a=1");
}

#[tokio::test]
async fn forwards_a_post_body() {
    let upstream = TestUpstream::start(OK).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;
    let reply = send_raw(
        proxy.addr,
        &format!("POST /submit HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello"),
    )
    .await;
    assert_eq!(reply.status, 200);
    let recorded = upstream.requests.lock().unwrap();
    assert_eq!(recorded[0].method, "POST");
    assert_eq!(recorded[0].body, b"hello");
}

#[tokio::test]
async fn path_and_query_rewriting_follows_the_route() {
    let upstream = TestUpstream::start(OK).await;
    let mut stripped = route(1, "/api");
    stripped.strip_prefix = true;
    let mut prefixed = route(2, "/shop");
    prefixed.strip_prefix = true;
    prefixed.upstream_prefix = UpstreamPrefix::parse("/v2").unwrap();
    let mut only_prefixed = route(3, "/keep");
    only_prefixed.upstream_prefix = UpstreamPrefix::parse("/v3").unwrap();
    let proxy = proxy_to(
        &upstream,
        vec![route(4, "/"), stripped, prefixed, only_prefixed],
    )
    .await;

    let cases = [
        ("/api/users?x=1&y=%20z", "/users?x=1&y=%20z"),
        ("/api", "/"),
        ("/shop/cart", "/v2/cart"),
        ("/keep/x?q", "/v3/keep/x?q"),
        ("/other/%2e%2e/a%2Fb", "/other/%2e%2e/a%2Fb"),
    ];
    for (requested, expected) in cases {
        get(proxy.addr, HOST, requested, "").await;
        assert_eq!(upstream.last_path(), expected, "{requested}");
    }
}

// ---- headers ------------------------------------------------------------

#[tokio::test]
async fn the_upstream_sees_forwarded_headers_and_the_request_id() {
    let upstream = TestUpstream::start(OK).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let reply = get(
        proxy.addr,
        HOST,
        "/",
        "X-Forwarded-For: 9.9.9.9\r\nX-Forwarded-Proto: https\r\nX-Request-Id: trace-7\r\n",
    )
    .await;
    let seen = upstream.last_headers();
    assert_eq!(
        value(&seen, "x-forwarded-for").as_deref(),
        Some("127.0.0.1")
    );
    assert_eq!(value(&seen, "x-forwarded-proto").as_deref(), Some("http"));
    assert_eq!(value(&seen, "x-forwarded-host").as_deref(), Some(HOST));
    assert_eq!(value(&seen, "x-request-id").as_deref(), Some("trace-7"));
    assert_eq!(reply.header("x-request-id"), Some("trace-7"));
}

#[tokio::test]
async fn a_generated_request_id_reaches_both_the_upstream_and_the_client() {
    let upstream = TestUpstream::start(OK).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;
    let reply = get(proxy.addr, HOST, "/", "").await;
    let upstream_id = value(&upstream.last_headers(), "x-request-id").unwrap();
    assert_eq!(upstream_id.len(), 32);
    assert_eq!(reply.header("x-request-id"), Some(upstream_id.as_str()));
}

#[tokio::test]
async fn trusted_proxies_have_their_forwarded_headers_extended() {
    let upstream = TestUpstream::start(OK).await;
    let proxy = proxy_with(&upstream, vec![route(1, "/")], &["127.0.0.1/32"]).await;
    get(
        proxy.addr,
        HOST,
        "/",
        "X-Forwarded-For: 9.9.9.9\r\nX-Forwarded-Proto: https\r\n",
    )
    .await;
    let seen = upstream.last_headers();
    assert_eq!(
        value(&seen, "x-forwarded-for").as_deref(),
        Some("9.9.9.9, 127.0.0.1")
    );
    assert_eq!(value(&seen, "x-forwarded-proto").as_deref(), Some("https"));
}

#[tokio::test]
async fn preserve_host_keeps_or_rewrites_the_host_header() {
    let upstream = TestUpstream::start(OK).await;
    let mut rewriting = route(2, "/rewrite");
    rewriting.preserve_host = false;
    let proxy = proxy_to(&upstream, vec![route(1, "/"), rewriting]).await;

    get(proxy.addr, HOST, "/", "").await;
    assert_eq!(
        value(&upstream.last_headers(), "host").as_deref(),
        Some(HOST)
    );

    get(proxy.addr, HOST, "/rewrite", "").await;
    let expected = upstream.addr.to_string();
    assert_eq!(value(&upstream.last_headers(), "host"), Some(expected));
}

#[tokio::test]
async fn hop_by_hop_headers_are_stripped_in_both_directions() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: vec![
            ("keep-alive", "timeout=5"),
            ("connection", "x-upstream-private"),
            ("x-upstream-private", "secret"),
            ("x-public", "visible"),
        ],
        body: "ok",
    })
    .await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let reply = get(
        proxy.addr,
        HOST,
        "/",
        "Keep-Alive: 300\r\nProxy-Authorization: Basic x\r\nTE: trailers\r\nConnection: x-client-private\r\nX-Client-Private: secret\r\nX-Client-Public: visible\r\n",
    )
    .await;

    let seen = upstream.last_headers();
    for gone in [
        "keep-alive",
        "proxy-authorization",
        "te",
        "x-client-private",
    ] {
        assert!(!seen.contains_key(gone), "{gone} leaked upstream");
    }
    assert_eq!(value(&seen, "x-client-public").as_deref(), Some("visible"));

    assert_eq!(reply.header("x-public"), Some("visible"));
    assert!(reply.header("x-upstream-private").is_none());
    assert!(reply.header("keep-alive").is_none());
}

// ---- errors -------------------------------------------------------------

#[tokio::test]
async fn a_slow_upstream_is_a_504() {
    let upstream = TestUpstream::start(Behavior::Sleep(Duration::from_secs(5))).await;
    let mut slow = route(1, "/");
    slow.response_timeout = Duration::from_millis(150);
    let proxy = proxy_to(&upstream, vec![slow]).await;

    let started = std::time::Instant::now();
    let reply = get(proxy.addr, HOST, "/", "").await;
    assert_eq!(reply.status, 504);
    assert_eq!(reply.body, "gateway timeout\n");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "timeout was not enforced"
    );
}

#[tokio::test]
async fn a_refused_connection_is_a_502() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, addr, vec![route(1, "/")])],
    };
    let proxy = Harness::proxying(config, &[]).await;

    let reply = get(proxy.addr, HOST, "/", "Accept: application/json\r\n").await;
    assert_eq!(reply.status, 502);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert!(reply.body.contains("bad_gateway"), "{}", reply.body);
    assert!(reply.header("x-request-id").is_some());
}

// ---- P1: pooling, streaming, cancellation -------------------------------

#[tokio::test]
async fn upstream_connections_are_pooled_and_reused() {
    let upstream = TestUpstream::start(OK).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;
    for _ in 0..5 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(upstream.request_count(), 5);
    assert_eq!(upstream.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_large_post_body_is_streamed_through() {
    const SIZE: usize = 64 * 1024 * 1024;
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {SIZE}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let chunk = vec![7u8; 1024 * 1024];
    for _ in 0..(SIZE / chunk.len()) {
        stream.write_all(&chunk).await.unwrap();
    }
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();

    assert_eq!(parse_reply(bytes).status, 200);
    assert_eq!(upstream.body_bytes.load(Ordering::SeqCst), SIZE);
}

#[tokio::test]
async fn the_request_body_reaches_the_upstream_while_the_client_is_still_sending() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    let head = format!(
        "POST /upload HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 2000\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(&[b'a'; 1000]).await.unwrap();

    timeout(Duration::from_secs(2), upstream.first_chunk.notified())
        .await
        .expect("the first half should be streamed before the second is sent");

    stream.write_all(&[b'b'; 1000]).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    assert_eq!(parse_reply(bytes).status, 200);
    assert_eq!(upstream.body_bytes.load(Ordering::SeqCst), 2000);
}

#[tokio::test]
async fn a_chunked_request_body_is_forwarded() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "POST /c HTTP/1.1\r\nHost: {HOST}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(upstream.body_bytes.load(Ordering::SeqCst), 11);
}

#[tokio::test]
async fn expect_100_continue_is_honored() {
    let upstream = TestUpstream::start(Behavior::CountBody).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    let head = format!(
        "POST /e HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: 5\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();

    let mut seen = Vec::new();
    let mut buf = [0u8; 256];
    while !String::from_utf8_lossy(&seen).contains("100 Continue") {
        let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("expected an interim 100 Continue")
            .unwrap();
        assert!(n > 0, "connection closed before 100 Continue");
        seen.extend_from_slice(&buf[..n]);
    }
    stream.write_all(b"hello").await.unwrap();
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await.unwrap();
    assert!(String::from_utf8_lossy(&rest).contains("200 OK"));
    assert_eq!(upstream.body_bytes.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn a_chunked_response_is_streamed_not_buffered() {
    let upstream = TestUpstream::start(Behavior::StreamResponse).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    let request = format!("GET /s HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();

    timeout(Duration::from_secs(2), upstream.stream_started.notified())
        .await
        .unwrap();
    let mut sender = upstream.stream_sender.lock().unwrap().take().unwrap();
    sender
        .send_data(Bytes::from_static(b"first"))
        .await
        .unwrap();

    // The first chunk must reach the client while the upstream body is still open.
    let mut seen = Vec::new();
    let mut buf = [0u8; 512];
    while !String::from_utf8_lossy(&seen).contains("first") {
        let n = timeout(Duration::from_secs(2), stream.read(&mut buf))
            .await
            .expect("first chunk was not streamed through")
            .unwrap();
        assert!(n > 0, "closed before the first chunk");
        seen.extend_from_slice(&buf[..n]);
    }

    sender
        .send_data(Bytes::from_static(b"second"))
        .await
        .unwrap();
    drop(sender);
    stream.read_to_end(&mut seen).await.unwrap();
    let text = String::from_utf8_lossy(&seen);
    assert!(text.contains("second"), "{text}");
    assert!(text.ends_with("0\r\n\r\n"), "{text}");
}

#[tokio::test]
async fn a_client_disconnect_cancels_the_upstream_request() {
    let upstream = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let proxy = proxy_to(&upstream, vec![route(1, "/")]).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    let request = format!("GET /hang HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    assert!(eventually(|| upstream.connections.load(Ordering::SeqCst) == 1).await);

    drop(stream);
    timeout(Duration::from_secs(3), upstream.dropped.notified())
        .await
        .expect("the upstream request should be cancelled when the client leaves");
}

#[tokio::test]
async fn one_client_is_reused_across_routes_with_the_same_connect_timeout() {
    let upstream = Arc::new(TestUpstream::start(OK).await);
    let proxy = proxy_to(&upstream, vec![route(1, "/"), route(2, "/a")]).await;
    get(proxy.addr, HOST, "/", "").await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    get(proxy.addr, HOST, "/a", "").await;
    assert_eq!(upstream.connections.load(Ordering::SeqCst), 1);
}
