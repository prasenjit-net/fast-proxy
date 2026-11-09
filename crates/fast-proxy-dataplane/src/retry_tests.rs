//! Retries end to end: what is retried, what is not, and the safety rails
//! (attempt limit, group budget, upgrades).

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{Config, GroupPolicy, RetryBudget, RetryPolicy, SubsystemConfig};

use crate::test_support::{get, route, send_raw, site_to_group, Harness};
use crate::test_upstream::{Behavior, TestUpstream};
use crate::upgrade_tests::connect;

const HOST: &str = "shop.example.com";
const OK: Behavior = Behavior::Respond {
    status: 200,
    headers: Vec::new(),
    body: "ok",
};

async fn backends(count: usize) -> Vec<Arc<TestUpstream>> {
    let mut started = Vec::new();
    for _ in 0..count {
        started.push(TestUpstream::start(OK).await);
    }
    started
}

fn site(upstreams: &[Arc<TestUpstream>], policy: GroupPolicy) -> SubsystemConfig {
    let addrs: Vec<_> = upstreams.iter().map(|u| u.addr).collect();
    site_to_group(1, "shop", HOST, &addrs, policy, vec![route(1, "/")])
}

async fn proxy(site: SubsystemConfig) -> Harness {
    Harness::proxying(
        Config {
            subsystems: vec![site],
        },
        &[],
    )
    .await
}

async fn kill(upstream: &TestUpstream) {
    upstream.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

async fn post(proxy: &Harness, body: &str) -> crate::test_support::Reply {
    send_raw(
        proxy.addr,
        &format!(
            "POST /submit HTTP/1.1\r\nHost: {HOST}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

fn metric_line(proxy: &Harness, name: &str) -> Option<f64> {
    proxy
        .metrics
        .render()
        .lines()
        .find(|line| line.starts_with(name))
        .and_then(|line| line.rsplit(' ').next()?.parse().ok())
}

#[tokio::test]
async fn killing_one_of_two_backends_causes_no_client_errors() {
    let upstreams = backends(2).await;
    let proxy = proxy(site(&upstreams, GroupPolicy::default())).await;
    kill(&upstreams[0]).await;
    for _ in 0..40 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert!(
        metric_line(
            &proxy,
            "fastproxy_upstream_retries_total{reason=\"connect\""
        )
        .unwrap_or(0.0)
            >= 1.0
    );
}

#[tokio::test]
async fn a_post_is_retried_after_a_connect_failure_with_its_body_intact() {
    let upstreams = backends(2).await;
    let proxy = proxy(site(&upstreams, GroupPolicy::default())).await;
    kill(&upstreams[0]).await;
    for round in 0..6 {
        let reply = post(&proxy, &format!("payload-{round}")).await;
        assert_eq!(reply.status, 200, "round {round}");
    }
    let recorded = upstreams[1].requests.lock().unwrap();
    let bodies: Vec<_> = recorded
        .iter()
        .map(|r| String::from_utf8_lossy(&r.body).to_string())
        .collect();
    for round in 0..6 {
        assert!(bodies.contains(&format!("payload-{round}")), "{bodies:?}");
    }
}

#[tokio::test]
async fn a_get_is_retried_when_the_upstream_answers_503() {
    let upstreams = backends(2).await;
    let proxy = proxy(site(&upstreams, GroupPolicy::default())).await;
    upstreams[0].fail_with(503);
    for _ in 0..2 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert!(
        upstreams[0].request_count() >= 1,
        "the sick endpoint was tried first"
    );
    assert!(
        metric_line(
            &proxy,
            "fastproxy_upstream_retries_total{reason=\"bad_status\""
        )
        .unwrap_or(0.0)
            >= 1.0
    );
}

#[tokio::test]
async fn a_post_is_not_retried_when_the_upstream_answers_503() {
    let upstreams = backends(2).await;
    let proxy = proxy(site(&upstreams, GroupPolicy::default())).await;
    upstreams[0].fail_with(503);
    // Round robin starts at the sick endpoint: its 503 is the final answer.
    assert_eq!(post(&proxy, "x").await.status, 503);
    assert_eq!(
        upstreams[1].request_count(),
        0,
        "the POST was not replayed elsewhere"
    );
}

#[tokio::test]
async fn max_attempts_of_one_turns_retries_off() {
    let upstreams = backends(2).await;
    let mut config = site(&upstreams, GroupPolicy::default());
    config.routes[0].retry = RetryPolicy {
        max_attempts: 1,
        ..RetryPolicy::default()
    };
    let proxy = proxy(config).await;
    kill(&upstreams[0]).await;
    let mut statuses = Vec::new();
    for _ in 0..2 {
        statuses.push(get(proxy.addr, HOST, "/", "").await.status);
    }
    assert!(statuses.contains(&502), "{statuses:?}");
}

#[tokio::test]
async fn timeouts_are_retried_only_when_the_route_opts_in() {
    for (on_timeout, expected) in [(false, 504), (true, 200)] {
        let hung = TestUpstream::start(Behavior::HoldUntilDropped).await;
        let healthy = TestUpstream::start(OK).await;
        let upstreams = [hung, healthy];
        let mut config = site(&upstreams, GroupPolicy::default());
        config.routes[0].response_timeout = Duration::from_millis(150);
        config.routes[0].retry = RetryPolicy {
            on_timeout,
            ..RetryPolicy::default()
        };
        let proxy = proxy(config).await;
        // Round robin starts at the hanging endpoint.
        assert_eq!(
            get(proxy.addr, HOST, "/", "").await.status,
            expected,
            "on_timeout={on_timeout}"
        );
    }
}

#[tokio::test]
async fn an_exhausted_budget_refuses_retries_and_says_so() {
    let upstreams = backends(2).await;
    let policy = GroupPolicy {
        retry_budget: RetryBudget {
            percent: 0,
            min_per_sec: 0,
        },
        ..GroupPolicy::default()
    };
    let proxy = proxy(site(&upstreams, policy)).await;
    kill(&upstreams[0]).await;
    let mut statuses = Vec::new();
    for _ in 0..4 {
        statuses.push(get(proxy.addr, HOST, "/", "").await.status);
    }
    assert!(
        statuses.contains(&502),
        "no retry without budget: {statuses:?}"
    );
    assert!(
        metric_line(&proxy, "fastproxy_retry_budget_exhausted_total").unwrap_or(0.0) >= 1.0,
        "{}",
        proxy.metrics.render()
    );
}

#[tokio::test]
async fn the_access_log_shows_two_attempts_and_the_endpoint_that_answered() {
    let upstreams = backends(2).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("access.log");
    let config = Config {
        subsystems: vec![site(&upstreams, GroupPolicy::default())],
    };
    let proxy = Harness::proxying_with_log(config, &[], Some(&path)).await;
    kill(&upstreams[0]).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    proxy.access_log.flush().await;
    let log = std::fs::read_to_string(&path).unwrap();
    let line = log.lines().next().unwrap();
    assert!(line.contains("attempts=2"), "{line}");
    assert!(
        line.contains(&format!("upstream={}", upstreams[1].addr)),
        "{line}"
    );
}

#[tokio::test]
async fn an_upgrade_is_never_retried() {
    use futures_util::SinkExt;
    let dead = TestUpstream::start(OK).await;
    let alive = crate::test_upstream::WsEchoServer::start().await;
    let addrs = [dead.addr, alive.addr];
    let config = site_to_group(
        1,
        "shop",
        HOST,
        &addrs,
        GroupPolicy::default(),
        vec![route(1, "/")],
    );
    let proxy = proxy(config).await;
    kill(&dead).await;
    // Round robin offers the dead endpoint first; a retry would reach the
    // live one, so a failed handshake proves there was none.
    let stream = tokio::net::TcpStream::connect(proxy.addr).await.unwrap();
    let request = tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(
        format!("ws://{HOST}/chat"),
    )
    .unwrap();
    let handshake =
        tokio_tungstenite::client_async(request, tokio_tungstenite::MaybeTlsStream::Plain(stream))
            .await;
    assert!(
        handshake.is_err(),
        "the first attempt's failure is final for an upgrade"
    );
    // The live endpoint still works for the next connection.
    let mut socket = connect(&proxy).await;
    socket
        .send(tokio_tungstenite::tungstenite::Message::text("hi"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_retry_never_goes_back_to_the_endpoint_that_just_failed() {
    use fast_proxy_model::{HashKey, LbAlgorithm, LbPolicy};
    // Consistent hashing sends one client to the same endpoint every time, so
    // without exclusion the retry would hit the dead endpoint again.
    let upstreams = backends(2).await;
    let policy = GroupPolicy {
        lb: LbPolicy {
            algorithm: LbAlgorithm::ConsistentHash,
            hash_key: HashKey::ClientIp,
        },
        ..GroupPolicy::default()
    };
    let proxy = proxy(site(&upstreams, policy)).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    let owner = usize::from(upstreams[0].request_count() == 0);
    kill(&upstreams[owner]).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
}

#[tokio::test]
async fn the_in_flight_count_follows_the_request_to_the_retried_endpoint() {
    let dead = TestUpstream::start(OK).await;
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_millis(400))).await;
    let upstreams = [dead.clone(), slow.clone()];
    let proxy = proxy(site(&upstreams, GroupPolicy::default())).await;
    kill(&dead).await;

    let addr = proxy.addr;
    let pending = tokio::spawn(async move { get(addr, HOST, "/", "").await });
    let snapshot = proxy.holder.load();
    let candidates = &snapshot
        .subsystems
        .values()
        .next()
        .unwrap()
        .upstreams
        .values()
        .next()
        .unwrap()
        .candidates;
    let retried = candidates[1].runtime.clone();
    assert!(crate::test_support::eventually(move || retried.in_flight() == 1).await);
    assert_eq!(
        candidates[0].runtime.in_flight(),
        0,
        "the failed endpoint let go"
    );
    assert_eq!(pending.await.unwrap().status, 200);
}
