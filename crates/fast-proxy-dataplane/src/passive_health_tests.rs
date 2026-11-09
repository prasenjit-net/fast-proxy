//! Passive health end to end: failures seen by the proxy take an endpoint
//! out of rotation, and a revived endpoint returns by itself.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::{monotonic_ms, EndpointRuntime};
use fast_proxy_model::{Config, GroupPolicy, PassiveHealth};

use crate::test_support::{get, route, site_to_group, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const HOST: &str = "shop.example.com";
const OK: Behavior = Behavior::Respond {
    status: 200,
    headers: Vec::new(),
    body: "ok",
};

fn policy(failures: u32, base_ms: u64) -> GroupPolicy {
    GroupPolicy {
        passive: PassiveHealth {
            consecutive_failures: failures,
            base_ejection: Duration::from_millis(base_ms),
            max_ejection: Duration::from_millis(base_ms * 8),
            max_ejection_percent: 50,
        },
        ..GroupPolicy::default()
    }
}

async fn pair(policy: GroupPolicy) -> (Vec<Arc<TestUpstream>>, Harness) {
    let backends = vec![TestUpstream::start(OK).await, TestUpstream::start(OK).await];
    let proxy = proxy_over(&backends, policy).await;
    (backends, proxy)
}

async fn proxy_over(backends: &[Arc<TestUpstream>], policy: GroupPolicy) -> Harness {
    let addrs: Vec<_> = backends.iter().map(|b| b.addr).collect();
    let site = site_to_group(1, "shop", HOST, &addrs, policy, vec![route(1, "/")]);
    Harness::proxying(
        Config {
            subsystems: vec![site],
        },
        &[],
    )
    .await
}

fn runtime(proxy: &Harness, index: usize) -> Arc<EndpointRuntime> {
    let snapshot = proxy.holder.load();
    let subsystem = snapshot.subsystems.values().next().unwrap();
    subsystem.upstreams.values().next().unwrap().candidates[index]
        .runtime
        .clone()
}

/// Sends requests until `done` holds, up to a bound.
async fn drive(proxy: &Harness, mut done: impl FnMut() -> bool) -> Vec<u16> {
    let mut statuses = Vec::new();
    for _ in 0..40 {
        if done() {
            break;
        }
        statuses.push(get(proxy.addr, HOST, "/", "").await.status);
    }
    statuses
}

#[tokio::test]
async fn a_dead_backend_is_ejected_after_the_configured_consecutive_failures() {
    let (backends, proxy) = pair(policy(3, 60_000)).await;
    backends[0].kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let dead = runtime(&proxy, 0);

    // Each failed attempt is retried on the healthy endpoint, so clients see
    // no errors while the dead one collects its three strikes.
    let statuses = drive(&proxy, || dead.is_ejected(monotonic_ms())).await;
    assert_eq!(
        statuses.len(),
        3,
        "ejected after exactly three requests: {statuses:?}"
    );
    assert!(statuses.iter().all(|s| *s == 200), "{statuses:?}");

    for _ in 0..10 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert!(proxy
        .metrics
        .render()
        .contains(r#"fastproxy_endpoint_ejections_total{endpoint="127.0.0.1:"#));
}

#[tokio::test]
async fn a_revived_backend_returns_to_rotation_after_its_ejection_expires() {
    let (backends, proxy) = pair(policy(2, 150)).await;
    backends[0].kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let dead = runtime(&proxy, 0);
    drive(&proxy, || dead.is_ejected(monotonic_ms())).await;

    backends[0].revive().await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    let before = backends[0].request_count();
    for _ in 0..10 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert!(
        backends[0].request_count() > before,
        "the revived backend serves again"
    );
    assert!(!dead.is_ejected(monotonic_ms()));
}

#[tokio::test]
async fn a_failed_trial_ejects_again_and_only_one_request_is_risked() {
    let (backends, proxy) = pair(policy(2, 150)).await;
    backends[0].kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let dead = runtime(&proxy, 0);
    drive(&proxy, || dead.is_ejected(monotonic_ms())).await;

    tokio::time::sleep(Duration::from_millis(250)).await;
    let connect_failures = |proxy: &Harness| {
        proxy
            .metrics
            .render()
            .lines()
            .filter(|line| line.contains(r#"outcome="connect""#))
            .filter_map(|line| line.rsplit(' ').next()?.parse::<u64>().ok())
            .sum::<u64>()
    };
    let before = connect_failures(&proxy);
    for _ in 0..10 {
        assert_eq!(
            get(proxy.addr, HOST, "/", "").await.status,
            200,
            "retries hide the trial"
        );
    }
    assert_eq!(
        connect_failures(&proxy) - before,
        1,
        "exactly one trial request was risked"
    );
    assert!(dead.is_ejected(monotonic_ms()));
}

#[tokio::test]
async fn gateway_statuses_count_as_failures_but_a_plain_500_does_not() {
    let (backends, proxy) = pair(policy(3, 60_000)).await;
    backends[0].fail_with(500);
    for _ in 0..20 {
        get(proxy.addr, HOST, "/", "").await;
    }
    assert!(
        !runtime(&proxy, 0).is_ejected(monotonic_ms()),
        "500 is not a health signal"
    );

    backends[0].fail_with(503);
    let sick = runtime(&proxy, 0);
    drive(&proxy, || sick.is_ejected(monotonic_ms())).await;
    assert!(sick.is_ejected(monotonic_ms()), "503 is");
}

#[tokio::test]
async fn a_single_endpoint_is_never_ejected_however_often_it_fails() {
    let backend = TestUpstream::start(OK).await;
    let proxy = proxy_over(std::slice::from_ref(&backend), policy(2, 60_000)).await;
    backend.fail_with(503);
    for _ in 0..10 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 503);
    }
    assert!(!runtime(&proxy, 0).is_ejected(monotonic_ms()));
    backend.recover();
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
}

#[tokio::test]
async fn upstream_requests_are_counted_by_outcome() {
    let backend = TestUpstream::start(OK).await;
    let proxy = proxy_over(std::slice::from_ref(&backend), GroupPolicy::default()).await;
    get(proxy.addr, HOST, "/", "").await;
    backend.fail_with(503);
    get(proxy.addr, HOST, "/", "").await;
    let text = proxy.metrics.render();
    assert!(
        text.contains(r#"outcome="success""#) && text.contains(r#"outcome="bad_status""#),
        "{text}"
    );
}

#[tokio::test]
async fn serving_in_panic_mode_is_counted() {
    let (_backends, proxy) = pair(GroupPolicy::default()).await;
    for index in 0..2 {
        runtime(&proxy, index).record_probe(false, 1, 1);
    }
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    assert!(proxy
        .metrics
        .render()
        .contains(r#"fastproxy_upstream_panic_total{subsystem="shop",upstream="backend"} 1"#));
}

#[tokio::test]
async fn a_client_leaving_during_the_trial_does_not_leave_the_endpoint_stuck() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;

    let hanging = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let healthy = TestUpstream::start(OK).await;
    let backends = [hanging.clone(), healthy];
    let proxy = proxy_over(&backends, policy(2, 150)).await;
    let flaky = runtime(&proxy, 0);

    hanging.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    drive(&proxy, || flaky.is_ejected(monotonic_ms())).await;
    hanging.revive().await;
    tokio::time::sleep(Duration::from_millis(250)).await;

    // Keep sending requests that the client abandons until one becomes the
    // trial on the hanging endpoint.
    let request = format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n");
    for _ in 0..10 {
        let mut client = TcpStream::connect(proxy.addr).await.unwrap();
        client.write_all(request.as_bytes()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let was_trial = hanging.request_count() > 0;
        drop(client);
        if was_trial {
            break;
        }
    }
    assert!(
        hanging.request_count() > 0,
        "a request reached the hanging endpoint"
    );
    let watched = flaky.clone();
    assert!(
        crate::test_support::eventually(move || watched.is_routable(monotonic_ms())).await,
        "the abandoned trial was released"
    );
}

#[tokio::test]
async fn a_response_timeout_counts_against_the_endpoint() {
    let hung = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let mut site = site_to_group(
        1,
        "shop",
        HOST,
        &[hung.addr],
        GroupPolicy::default(),
        vec![route(1, "/")],
    );
    site.routes[0].response_timeout = Duration::from_millis(150);
    let proxy = Harness::proxying(
        Config {
            subsystems: vec![site],
        },
        &[],
    )
    .await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 504);
    assert!(proxy.metrics.render().contains(r#"outcome="timeout""#));
}
