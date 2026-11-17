//! Rate limits and exact endpoint caps through the real pipeline (ADRs 0030,
//! 0031), and the refusal reason every limit leaves in the log and metrics.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{
    Config, GroupPolicy, RateLimitHeader, RateLimitKey, RateLimitPolicy, DEFAULT_IPV6_PREFIX,
};

use crate::test_support::{get, route, site_to, site_to_group, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const HOST: &str = "shop.example.com";

fn ok() -> Behavior {
    Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    }
}

fn policy(burst: u32, key: RateLimitKey) -> RateLimitPolicy {
    RateLimitPolicy {
        rate: 1,
        period: Duration::from_secs(60),
        burst,
        key,
        ipv6_prefix: DEFAULT_IPV6_PREFIX,
    }
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

#[tokio::test]
async fn past_its_burst_a_client_gets_429_with_retry_after_and_the_upstream_sees_nothing() {
    let upstream = TestUpstream::start(ok()).await;
    let mut shop = site_to(1, "shop", HOST, upstream.addr, vec![route(1, "/")]);
    shop.rate_limit = Some(policy(2, RateLimitKey::Client));
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("access.log");
    let proxy = Harness::proxying_with_log(
        Config {
            subsystems: vec![shop],
        },
        &[],
        Some(&log),
    )
    .await;

    for _ in 0..2 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    let limited = get(proxy.addr, HOST, "/", "").await;
    assert_eq!(limited.status, 429);
    let retry_after: u64 = limited.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=60).contains(&retry_after), "{retry_after}");
    assert_eq!(
        upstream.request_count(),
        2,
        "the refused request was not forwarded"
    );

    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_rate_limited_total{route="/",subsystem="shop"}"#
        ),
        Some(1.0)
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_requests_refused_total{reason="rate_limited"}"#
        ),
        Some(1.0)
    );
    proxy.access_log.flush().await;
    let lines = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        lines.matches(" refused=rate_limited ").count(),
        1,
        "{lines}"
    );
}

#[tokio::test]
async fn a_header_policy_gives_each_key_its_own_budget() {
    let upstream = TestUpstream::start(ok()).await;
    let mut api = route(1, "/");
    api.rate_limit = Some(policy(
        1,
        RateLimitKey::Header(RateLimitHeader::parse("h", "x-api-key").unwrap()),
    ));
    let proxy = Harness::proxying(
        Config {
            subsystems: vec![site_to(1, "shop", HOST, upstream.addr, vec![api])],
        },
        &[],
    )
    .await;
    let with_key = |key: &'static str| format!("X-Api-Key: {key}\r\n");
    assert_eq!(get(proxy.addr, HOST, "/", &with_key("a")).await.status, 200);
    assert_eq!(get(proxy.addr, HOST, "/", &with_key("a")).await.status, 429);
    assert_eq!(get(proxy.addr, HOST, "/", &with_key("b")).await.status, 200);
}

#[tokio::test]
async fn concurrent_requests_never_pass_an_endpoints_cap() {
    let upstream = TestUpstream::start(Behavior::Sleep(Duration::from_millis(500))).await;
    let config = Config {
        subsystems: vec![site_to_group(
            1,
            "shop",
            HOST,
            &[upstream.addr],
            GroupPolicy {
                max_requests: Some(1),
                ..GroupPolicy::default()
            },
            vec![route(1, "/")],
        )],
    };
    let proxy = Arc::new(Harness::proxying(config, &[]).await);
    let requests: Vec<_> = (0..10)
        .map(|_| {
            let addr = proxy.addr;
            tokio::spawn(async move { get(addr, HOST, "/", "").await })
        })
        .collect();
    let mut statuses = Vec::new();
    for request in requests {
        statuses.push(request.await.unwrap().status);
    }
    statuses.sort_unstable();
    assert_eq!(
        statuses.iter().filter(|status| **status == 200).count(),
        1,
        "{statuses:?}"
    );
    assert!(statuses.iter().all(|status| [200, 503].contains(status)));
    assert_eq!(
        upstream.request_count(),
        1,
        "the upstream never had two at once"
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_requests_refused_total{reason="endpoint_capacity"}"#
        ),
        Some(9.0)
    );
}

#[tokio::test]
async fn a_full_endpoint_sends_the_request_to_one_with_room() {
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_millis(400))).await;
    let fast = TestUpstream::start(ok()).await;
    let config = Config {
        subsystems: vec![site_to_group(
            1,
            "shop",
            HOST,
            &[slow.addr, fast.addr],
            GroupPolicy {
                max_requests: Some(1),
                ..GroupPolicy::default()
            },
            vec![route(1, "/")],
        )],
    };
    let proxy = Arc::new(Harness::proxying(config, &[]).await);
    let requests: Vec<_> = (0..2)
        .map(|_| {
            let addr = proxy.addr;
            tokio::spawn(async move { get(addr, HOST, "/", "").await.status })
        })
        .collect();
    for request in requests {
        assert_eq!(request.await.unwrap(), 200, "two endpoints, one place each");
    }
}
