//! In-flight and latency accounting, drain semantics, the access log's
//! `attempts` field, and the failure-on-demand test backend itself.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::EndpointRuntime;
use fast_proxy_model::{Config, EndpointState, GroupPolicy};
use fast_proxy_routing::compile;
use futures_util::SinkExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use crate::test_support::{eventually, get, route, site_to_group, Harness};
use crate::test_upstream::{Behavior, TestUpstream, WsEchoServer};
use crate::upgrade_tests::{connect, proxy_for};

const HOST: &str = "shop.example.com";
const OK: Behavior = Behavior::Respond {
    status: 200,
    headers: Vec::new(),
    body: "ok",
};

fn config_for(upstreams: &[&Arc<TestUpstream>]) -> Config {
    let addrs: Vec<_> = upstreams.iter().map(|u| u.addr).collect();
    Config {
        subsystems: vec![site_to_group(
            1,
            "shop",
            HOST,
            &addrs,
            GroupPolicy::default(),
            vec![route(1, "/")],
        )],
    }
}

fn runtime(proxy: &Harness, index: usize) -> Arc<EndpointRuntime> {
    let snapshot = proxy.holder.load();
    let upstream = snapshot
        .subsystems
        .values()
        .next()
        .unwrap()
        .upstreams
        .values()
        .next()
        .unwrap();
    upstream.candidates[index].runtime.clone()
}

async fn idle(runtime: &Arc<EndpointRuntime>) -> bool {
    let runtime = runtime.clone();
    eventually(move || runtime.in_flight() == 0).await
}

#[tokio::test]
async fn in_flight_is_held_during_a_request_and_released_after() {
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_millis(300))).await;
    let proxy = Harness::proxying(config_for(&[&slow]), &[]).await;
    let endpoint = runtime(&proxy, 0);

    let addr = proxy.addr;
    let pending = tokio::spawn(async move { get(addr, HOST, "/", "").await });
    let watched = endpoint.clone();
    assert!(eventually(move || watched.in_flight() == 1).await);
    assert_eq!(pending.await.unwrap().status, 200);
    assert!(idle(&endpoint).await, "released once the response finished");
}

#[tokio::test]
async fn in_flight_returns_to_zero_when_the_client_abandons_the_request() {
    let hung = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let proxy = Harness::proxying(config_for(&[&hung]), &[]).await;
    let endpoint = runtime(&proxy, 0);

    let mut client = TcpStream::connect(proxy.addr).await.unwrap();
    client
        .write_all(format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let watched = endpoint.clone();
    assert!(eventually(move || watched.in_flight() == 1).await);
    drop(client);
    assert!(idle(&endpoint).await);
}

#[tokio::test]
async fn an_open_websocket_tunnel_counts_as_in_flight_until_it_closes() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let endpoint = runtime(&proxy, 0);

    let mut socket = connect(&proxy).await;
    socket.send(Message::text("ping")).await.unwrap();
    assert!(endpoint.in_flight() >= 1, "the tunnel is in flight");
    socket.close(None).await.unwrap();
    drop(socket);
    assert!(idle(&endpoint).await);
}

#[tokio::test]
async fn a_response_updates_the_endpoints_latency_average() {
    let backend = TestUpstream::start(OK).await;
    let proxy = Harness::proxying(config_for(&[&backend]), &[]).await;
    let endpoint = runtime(&proxy, 0);
    assert_eq!(endpoint.ewma_us(), 0);
    get(proxy.addr, HOST, "/", "").await;
    // A real loopback round trip takes tens of microseconds at least; a
    // placeholder sample would be 0 or 1.
    assert!(endpoint.ewma_us() >= 10, "{}", endpoint.ewma_us());
}

#[tokio::test]
async fn draining_stops_new_requests_while_the_one_in_flight_completes() {
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_millis(400))).await;
    let fast = TestUpstream::start(OK).await;
    let config = config_for(&[&slow, &fast]);
    let proxy = Harness::proxying(config.clone(), &[]).await;

    let addr = proxy.addr;
    let in_flight = tokio::spawn(async move { get(addr, HOST, "/", "").await });
    let arrived = slow.clone();
    assert!(eventually(move || arrived.request_count() == 1).await);

    let mut draining = config;
    draining.subsystems[0].upstream_groups[0].endpoints[0].state = EndpointState::Draining;
    let carried = proxy.holder.load().runtime_registry();
    proxy.holder.swap(compile(2, &draining, &carried).unwrap());

    for _ in 0..5 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert_eq!(
        slow.request_count(),
        1,
        "no new request reached the draining endpoint"
    );
    assert_eq!(fast.request_count(), 5);
    assert_eq!(
        in_flight.await.unwrap().status,
        200,
        "the in-flight request finished"
    );
}

#[tokio::test]
async fn the_access_log_records_the_endpoint_and_the_attempt_count() {
    let backend = TestUpstream::start(OK).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let proxy = Harness::proxying_with_log(config_for(&[&backend]), &[], Some(&path)).await;
    get(proxy.addr, HOST, "/", "").await;
    get(proxy.addr, "unknown.example.com", "/", "").await;
    proxy.access_log.flush().await;

    let log = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    let served = format!("upstream={} attempts=1 ", backend.addr);
    assert!(lines[0].contains(&served), "{}", lines[0]);
    assert!(lines[1].contains("upstream=- attempts=0 "), "{}", lines[1]);
}

#[tokio::test]
async fn the_test_backend_can_be_killed_failed_and_revived_on_demand() {
    let backend = TestUpstream::start(OK).await;
    let proxy = Harness::proxying(config_for(&[&backend]), &[]).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);

    backend.fail_with(503);
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 503);
    backend.recover();
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);

    backend.kill();
    let mut refused = 0;
    for _ in 0..50 {
        if get(proxy.addr, HOST, "/", "").await.status == 502 {
            refused += 1;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(refused, 1, "a killed backend refuses connections");

    backend.revive().await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
}

#[tokio::test]
async fn a_request_whose_client_leaves_before_the_response_is_counted_and_logged_as_499() {
    let hung = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let proxy = Harness::proxying_with_log(config_for(&[&hung]), &[], Some(&path)).await;

    let mut client = TcpStream::connect(proxy.addr).await.unwrap();
    client
        .write_all(format!("GET /slow HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let arrived = hung.clone();
    assert!(eventually(move || arrived.request_count() == 1).await);
    drop(client);

    let metrics = proxy.metrics.clone();
    let counted = r#"fastproxy_requests_cancelled_total{route="/",subsystem="shop"} 1"#;
    assert!(
        eventually(move || metrics.render().contains(counted)).await,
        "counted as cancelled"
    );
    assert!(
        proxy.metrics.render().contains(
            r#"fastproxy_requests_total{route="/",status_class="4xx",subsystem="shop"} 1"#
        ),
        "and as a 4xx request"
    );
    proxy.access_log.flush().await;
    let log = std::fs::read_to_string(&path).unwrap();
    let line = log.lines().next().unwrap();
    assert!(line.contains("\"GET /slow HTTP/1.1\" 499 "), "{line}");
    assert!(
        line.contains(&format!("upstream={} attempts=1 ", hung.addr)),
        "the log shows where it was waiting: {line}"
    );
}

#[tokio::test]
async fn requests_that_complete_are_never_reported_as_cancelled() {
    let backend = TestUpstream::start(OK).await;
    let proxy = Harness::proxying(config_for(&[&backend]), &[]).await;
    for _ in 0..3 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    let unmatched = get(proxy.addr, "unknown.example.com", "/", "").await;
    assert_eq!(unmatched.status, 404);
    assert!(!proxy
        .metrics
        .render()
        .contains("fastproxy_requests_cancelled_total{"));
}
