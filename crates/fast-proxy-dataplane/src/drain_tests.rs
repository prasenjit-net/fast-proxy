//! Graceful shutdown: stop accepting, let in-flight work finish within the
//! grace period, then close whatever is left.

use std::time::{Duration, Instant};

use fast_proxy_model::Config;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

use crate::test_support::{eventually, parse_reply, route, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream, WsEchoServer};
use crate::upgrade_tests::{connect, proxy_for};
use crate::{TrustedProxies, UpstreamClient};

const HOST: &str = "shop.example.com";

async fn proxy_to(upstream: &TestUpstream) -> Harness {
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![route(1, "/")])],
    };
    Harness::start(config, UpstreamClient::new(TrustedProxies::default())).await
}

fn request() -> String {
    format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n")
}

#[tokio::test]
async fn an_in_flight_request_finishes_during_a_graceful_shutdown() {
    let upstream = TestUpstream::start(Behavior::Sleep(Duration::from_millis(400))).await;
    let proxy = proxy_to(&upstream).await;
    let addr = proxy.addr;

    let client = tokio::spawn(async move {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(request().as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        parse_reply(bytes)
    });
    // The upstream has the request and is now sleeping on it.
    assert!(eventually(|| upstream.request_count() == 1).await);
    tokio::time::sleep(Duration::from_millis(100)).await;

    proxy.shutdown.cancel();
    let report = proxy.plane.stopped(Duration::from_secs(5)).await;
    assert_eq!(report.forced_connections, 0);

    let reply = client.await.unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "slow");
    assert!(
        TcpStream::connect(addr).await.is_err(),
        "no new connections after shutdown"
    );
}

#[tokio::test]
async fn a_request_still_running_after_the_grace_period_is_cut_off() {
    let upstream = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let proxy = proxy_to(&upstream).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream.write_all(request().as_bytes()).await.unwrap();
    assert!(
        eventually(|| upstream
            .connections
            .load(std::sync::atomic::Ordering::SeqCst)
            == 1)
        .await
    );

    proxy.shutdown.cancel();
    let started = Instant::now();
    let report = proxy.plane.stopped(Duration::from_millis(250)).await;
    assert_eq!(report.forced_connections, 1);
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert!(started.elapsed() < Duration::from_secs(3));

    let mut bytes = Vec::new();
    let read = timeout(Duration::from_secs(2), stream.read_to_end(&mut bytes)).await;
    assert!(
        read.is_ok(),
        "the client's connection should have been closed"
    );
    assert!(bytes.is_empty(), "no response was ever produced");
}

#[tokio::test]
async fn idle_keep_alive_connections_close_immediately_on_shutdown() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let proxy = proxy_to(&upstream).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream.write_all(request().as_bytes()).await.unwrap();
    let mut seen = Vec::new();
    let mut buf = [0u8; 512];
    while !String::from_utf8_lossy(&seen).ends_with("ok") {
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        seen.extend_from_slice(&buf[..n]);
    }
    assert_eq!(
        proxy.plane.active_connections(),
        1,
        "the connection is idle but open"
    );

    proxy.shutdown.cancel();
    let started = Instant::now();
    let report = proxy.plane.stopped(Duration::from_secs(5)).await;
    assert_eq!(report.forced_connections, 0);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an idle connection must not hold up shutdown"
    );

    let mut rest = Vec::new();
    timeout(Duration::from_secs(1), stream.read_to_end(&mut rest))
        .await
        .expect("the idle connection should have been closed")
        .unwrap();
}

#[tokio::test]
async fn a_websocket_that_stays_open_is_closed_after_the_grace_period() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;
    socket.send(Message::text("hi")).await.unwrap();
    socket.next().await.unwrap().unwrap();

    proxy.shutdown.cancel();
    let report = proxy.plane.stopped(Duration::from_millis(250)).await;
    assert_eq!(report.forced_connections, 1);

    let ended = timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) => return,
                Some(Ok(message)) if message.is_close() => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the tunnel should be torn down");
}

#[tokio::test]
async fn a_websocket_closed_within_the_grace_period_drains_cleanly() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;
    socket.send(Message::text("hi")).await.unwrap();
    socket.next().await.unwrap().unwrap();

    proxy.shutdown.cancel();
    let draining = tokio::spawn(proxy.plane.stopped(Duration::from_secs(5)));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !draining.is_finished(),
        "the open tunnel should hold the drain"
    );

    socket.close(None).await.unwrap();
    drop(socket);
    let report = timeout(Duration::from_secs(3), draining)
        .await
        .expect("the drain should finish once the tunnel closes")
        .unwrap();
    assert_eq!(report.forced_connections, 0);
}

#[tokio::test]
async fn draining_an_idle_proxy_returns_immediately() {
    let upstream = TestUpstream::start(Behavior::Sleep(Duration::from_millis(1))).await;
    let proxy = proxy_to(&upstream).await;
    proxy.shutdown.cancel();
    let started = Instant::now();
    let report = proxy.plane.stopped(Duration::from_secs(30)).await;
    assert_eq!(report.forced_connections, 0);
    assert!(started.elapsed() < Duration::from_secs(1));
}
