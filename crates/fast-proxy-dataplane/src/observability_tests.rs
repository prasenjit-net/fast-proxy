//! Metrics and access-log behavior of the real proxy under known traffic.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

use bytes::Bytes;
use fast_proxy_model::{Config, SubsystemConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::test_support::{eventually, get, route, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const HOST: &str = "shop.example.com";

/// The value of the exposition line that starts with `prefix`.
fn metric(harness: &Harness, prefix: &str) -> Option<f64> {
    harness
        .metrics
        .render()
        .lines()
        .find(|line| line.starts_with(prefix))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

fn requests(harness: &Harness, subsystem: &str, route: &str, class: &str) -> f64 {
    let prefix = format!(
        "fastproxy_requests_total{{route=\"{route}\",status_class=\"{class}\",subsystem=\"{subsystem}\"}}"
    );
    metric(harness, &prefix).unwrap_or(0.0)
}

fn ok_upstream_behavior() -> Behavior {
    Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "hello world",
    }
}

fn shop_and_disabled(upstream: &TestUpstream) -> Config {
    let mut off: SubsystemConfig = site_to(
        2,
        "off",
        "off.example.com",
        upstream.addr,
        vec![route(2, "/")],
    );
    off.enabled = false;
    Config {
        subsystems: vec![
            site_to(
                1,
                "shop",
                HOST,
                upstream.addr,
                vec![route(1, "/"), route(3, "/api")],
            ),
            off,
        ],
    }
}

/// Parses a log line's `key=value` tail.
fn parse_fields(line: &str) -> HashMap<String, String> {
    line.split(' ')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

async fn log_lines(harness: &Harness, path: &std::path::Path) -> Vec<String> {
    harness.access_log.flush().await;
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(ToString::to_string)
        .collect()
}

// ---- metrics ------------------------------------------------------------

#[tokio::test]
async fn requests_are_counted_by_subsystem_route_and_status_class() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let proxy = Harness::proxying(shop_and_disabled(&upstream), &[]).await;

    for _ in 0..3 {
        get(proxy.addr, HOST, "/page", "").await;
    }
    get(proxy.addr, HOST, "/api/users", "").await;
    get(proxy.addr, "nobody.example.com", "/", "").await;
    get(proxy.addr, "off.example.com", "/", "").await;

    assert!(eventually(|| requests(&proxy, "shop", "/", "2xx") == 3.0).await);
    assert!(eventually(|| requests(&proxy, "shop", "/api", "2xx") == 1.0).await);
    assert!(eventually(|| requests(&proxy, "_unmatched", "_none", "4xx") == 1.0).await);
    assert!(eventually(|| requests(&proxy, "off", "_none", "5xx") == 1.0).await);
}

#[tokio::test]
async fn a_subsystem_without_a_matching_route_is_labelled_with_its_name_and_no_route() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let config = Config {
        subsystems: vec![site_to(
            1,
            "shop",
            HOST,
            upstream.addr,
            vec![route(1, "/api")],
        )],
    };
    let proxy = Harness::proxying(config, &[]).await;
    get(proxy.addr, HOST, "/elsewhere", "").await;
    assert!(eventually(|| requests(&proxy, "shop", "_none", "4xx") == 1.0).await);
}

#[tokio::test]
async fn durations_are_recorded_per_subsystem_and_route() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let proxy = Harness::proxying(shop_and_disabled(&upstream), &[]).await;
    get(proxy.addr, HOST, "/", "").await;
    get(proxy.addr, HOST, "/", "").await;
    let prefix = "fastproxy_request_duration_seconds_count{route=\"/\",subsystem=\"shop\"}";
    assert!(eventually(|| metric(&proxy, prefix) == Some(2.0)).await);
}

#[tokio::test]
async fn labels_never_contain_raw_hosts_or_paths() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let proxy = Harness::proxying(shop_and_disabled(&upstream), &[]).await;
    get(
        proxy.addr,
        "evil-host.example",
        "/secret/path?token=abc",
        "",
    )
    .await;
    get(proxy.addr, HOST, "/secret/other?token=abc", "").await;
    assert!(eventually(|| requests(&proxy, "_unmatched", "_none", "4xx") == 1.0).await);
    assert!(eventually(|| requests(&proxy, "shop", "/", "2xx") == 1.0).await);

    let text = proxy.metrics.render();
    for leaked in ["evil-host", "secret", "token", "abc"] {
        assert!(!text.contains(leaked), "{leaked} leaked into the metrics");
    }
}

#[tokio::test]
async fn upstream_failures_are_counted_by_kind() {
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed_addr = closed.local_addr().unwrap();
    drop(closed);
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_secs(5))).await;

    let mut slow_route = route(2, "/");
    slow_route.response_timeout = Duration::from_millis(150);
    let config = Config {
        subsystems: vec![
            site_to(
                1,
                "refused",
                "refused.example.com",
                closed_addr,
                vec![route(1, "/")],
            ),
            site_to(
                2,
                "sluggish",
                "slow.example.com",
                slow.addr,
                vec![slow_route],
            ),
        ],
    };
    let proxy = Harness::proxying(config, &[]).await;

    assert_eq!(
        get(proxy.addr, "refused.example.com", "/", "").await.status,
        502
    );
    assert_eq!(
        get(proxy.addr, "slow.example.com", "/", "").await.status,
        504
    );

    let errors = |kind: &str, subsystem: &str| {
        metric(
            &proxy,
            &format!(
                "fastproxy_upstream_errors_total{{kind=\"{kind}\",subsystem=\"{subsystem}\"}}"
            ),
        )
    };
    assert_eq!(errors("connect", "refused"), Some(1.0));
    assert_eq!(errors("timeout", "sluggish"), Some(1.0));
    assert_eq!(errors("timeout", "refused"), None);
    assert!(eventually(|| requests(&proxy, "refused", "/", "5xx") == 1.0).await);
    assert!(eventually(|| requests(&proxy, "sluggish", "/", "5xx") == 1.0).await);
}

#[tokio::test]
async fn active_gauges_follow_requests_and_connections() {
    let upstream = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![route(1, "/")])],
    };
    let proxy = Harness::proxying(config, &[]).await;
    assert_eq!(metric(&proxy, "fastproxy_active_requests"), Some(0.0));

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream
        .write_all(format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    assert!(eventually(|| upstream.connections.load(Ordering::SeqCst) == 1).await);
    assert_eq!(metric(&proxy, "fastproxy_active_requests"), Some(1.0));
    assert_eq!(metric(&proxy, "fastproxy_active_connections"), Some(1.0));

    drop(stream);
    assert!(eventually(|| metric(&proxy, "fastproxy_active_connections") == Some(0.0)).await);
    assert!(eventually(|| metric(&proxy, "fastproxy_active_requests") == Some(0.0)).await);
}

// ---- access log ---------------------------------------------------------

#[tokio::test]
async fn forwarded_requests_are_logged_with_every_data_plane_field() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let proxy = Harness::proxying_with_log(shop_and_disabled(&upstream), &[], Some(&path)).await;

    let reply = get(
        proxy.addr,
        HOST,
        "/api/users?page=2",
        "X-Request-Id: trace-9\r\n",
    )
    .await;
    assert_eq!(reply.body, "hello world");

    assert!(eventually(|| metric(&proxy, "fastproxy_active_requests") == Some(0.0)).await);
    let lines = log_lines(&proxy, &path).await;
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(
        line.contains("\"GET /api/users?page=2 HTTP/1.1\" 200"),
        "{line}"
    );

    let fields = parse_fields(line);
    assert_eq!(fields["plane"], "data");
    assert_eq!(fields["subsystem"], "shop");
    assert_eq!(fields["route"], "/api");
    assert_eq!(fields["upstream"], upstream.addr.to_string());
    assert_eq!(fields["request_id"], "trace-9");
    assert_eq!(fields["bytes_out"], "11");
    assert!(fields["upstream_ms"].parse::<f64>().unwrap() >= 0.0);
    let total: f64 = fields["total_ms"].parse().unwrap();
    assert!(total >= fields["upstream_ms"].parse::<f64>().unwrap());
}

#[tokio::test]
async fn unmatched_requests_are_logged_with_dashes() {
    let upstream = TestUpstream::start(ok_upstream_behavior()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let proxy = Harness::proxying_with_log(shop_and_disabled(&upstream), &[], Some(&path)).await;

    get(proxy.addr, "nobody.example.com", "/", "").await;
    assert!(eventually(|| metric(&proxy, "fastproxy_active_requests") == Some(0.0)).await);
    let lines = log_lines(&proxy, &path).await;
    let fields = parse_fields(&lines[0]);
    assert!(lines[0].contains("\" 404 "), "{}", lines[0]);
    assert_eq!(fields["subsystem"], "_unmatched");
    assert_eq!(fields["route"], "_none");
    assert_eq!(fields["upstream"], "-");
    assert_eq!(fields["upstream_ms"], "-");
    assert_eq!(fields["request_id"].len(), 32);
    assert_eq!(
        fields["bytes_out"], "13",
        "the body is 'unknown host' plus a newline"
    );
}

#[tokio::test]
async fn a_streamed_response_reports_all_the_bytes_that_were_sent() {
    let upstream = TestUpstream::start(Behavior::StreamResponse).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![route(1, "/")])],
    };
    let proxy = Harness::proxying_with_log(config, &[], Some(&path)).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream
        .write_all(
            format!("GET /s HTTP/1.1\r\nHost: {HOST}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(2), upstream.stream_started.notified())
        .await
        .unwrap();
    let mut sender = upstream.stream_sender.lock().unwrap().take().unwrap();
    sender
        .send_data(Bytes::from_static(b"first"))
        .await
        .unwrap();
    sender
        .send_data(Bytes::from_static(b"second"))
        .await
        .unwrap();
    drop(sender);
    let mut sink = Vec::new();
    stream.read_to_end(&mut sink).await.unwrap();

    assert!(eventually(|| metric(&proxy, "fastproxy_active_requests") == Some(0.0)).await);
    let lines = log_lines(&proxy, &path).await;
    assert_eq!(parse_fields(&lines[0])["bytes_out"], "11");
}

#[tokio::test]
async fn a_client_that_leaves_mid_response_is_still_logged_with_the_bytes_sent_so_far() {
    let upstream = TestUpstream::start(Behavior::StreamResponse).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![route(1, "/")])],
    };
    let proxy = Harness::proxying_with_log(config, &[], Some(&path)).await;

    let mut stream = TcpStream::connect(proxy.addr).await.unwrap();
    stream
        .write_all(format!("GET /s HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    timeout(Duration::from_secs(2), upstream.stream_started.notified())
        .await
        .unwrap();
    let mut sender = upstream.stream_sender.lock().unwrap().take().unwrap();
    sender
        .send_data(Bytes::from_static(b"first"))
        .await
        .unwrap();
    let mut buf = [0u8; 512];
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains("first") {
        let n = stream.read(&mut buf).await.unwrap();
        seen.extend_from_slice(&buf[..n]);
    }
    drop(stream);

    assert!(eventually(|| metric(&proxy, "fastproxy_active_requests") == Some(0.0)).await);
    let lines = log_lines(&proxy, &path).await;
    assert_eq!(lines.len(), 1);
    assert_eq!(parse_fields(&lines[0])["bytes_out"], "5");
    drop(sender);
}
