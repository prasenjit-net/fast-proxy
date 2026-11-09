//! End-to-end tests of endpoint selection: the real proxy spreading requests
//! over several in-process upstreams, and the failure policy when none is
//! eligible.

use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use fast_proxy_model::{
    Config, EndpointState, FailurePolicy, GroupPolicy, HashKey, LbAlgorithm, LbPolicy,
};

use crate::test_support::{get, route, site_to_group, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

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

fn addrs(upstreams: &[Arc<TestUpstream>]) -> Vec<SocketAddr> {
    upstreams.iter().map(|u| u.addr).collect()
}

fn policy(algorithm: LbAlgorithm) -> GroupPolicy {
    GroupPolicy {
        lb: LbPolicy {
            algorithm,
            hash_key: HashKey::ClientIp,
        },
        ..GroupPolicy::default()
    }
}

async fn proxy(upstreams: &[Arc<TestUpstream>], policy: GroupPolicy) -> Harness {
    proxy_config(upstreams, policy, |_| {}).await
}

async fn proxy_config(
    upstreams: &[Arc<TestUpstream>],
    policy: GroupPolicy,
    edit: impl FnOnce(&mut fast_proxy_model::SubsystemConfig),
) -> Harness {
    let mut site = site_to_group(
        1,
        "shop",
        HOST,
        &addrs(upstreams),
        policy,
        vec![route(1, "/")],
    );
    edit(&mut site);
    Harness::proxying(
        Config {
            subsystems: vec![site],
        },
        &[],
    )
    .await
}

fn hits(upstreams: &[Arc<TestUpstream>]) -> Vec<usize> {
    upstreams
        .iter()
        .map(|u| u.requests.lock().unwrap().len())
        .collect()
}

#[tokio::test]
async fn round_robin_spreads_requests_evenly_over_three_endpoints() {
    let upstreams = backends(3).await;
    let proxy = proxy(&upstreams, policy(LbAlgorithm::RoundRobin)).await;
    for _ in 0..30 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert_eq!(hits(&upstreams), [10, 10, 10]);
}

#[tokio::test]
async fn weighted_round_robin_follows_the_weights() {
    let upstreams = backends(2).await;
    let proxy = proxy_config(
        &upstreams,
        policy(LbAlgorithm::WeightedRoundRobin),
        |site| {
            site.upstream_groups[0].endpoints[0].weight = 3;
        },
    )
    .await;
    // Rebuild is not needed: the weight was set before compile.
    for _ in 0..40 {
        get(proxy.addr, HOST, "/", "").await;
    }
    assert_eq!(hits(&upstreams), [30, 10]);
}

#[tokio::test]
async fn consistent_hash_on_a_header_sends_one_key_to_one_endpoint() {
    let upstreams = backends(3).await;
    let mut hashing = policy(LbAlgorithm::ConsistentHash);
    hashing.lb.hash_key = HashKey::Header("x-user".into());
    let proxy = proxy(&upstreams, hashing).await;
    for _ in 0..12 {
        get(proxy.addr, HOST, "/", "x-user: alice\r\n").await;
    }
    let after_alice = hits(&upstreams);
    assert_eq!(
        after_alice.iter().filter(|h| **h == 12).count(),
        1,
        "{after_alice:?}"
    );
    for user in 0..40 {
        get(proxy.addr, HOST, "/", &format!("x-user: user{user}\r\n")).await;
    }
    assert!(
        hits(&upstreams).iter().all(|h| *h > 0),
        "many keys reach every endpoint"
    );
}

#[tokio::test]
async fn least_connections_and_p2c_serve_every_request() {
    for algorithm in [LbAlgorithm::LeastConnections, LbAlgorithm::P2cEwma] {
        let upstreams = backends(3).await;
        let proxy = proxy(&upstreams, policy(algorithm)).await;
        for _ in 0..30 {
            assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
        }
        assert_eq!(hits(&upstreams).iter().sum::<usize>(), 30, "{algorithm:?}");
    }
}

#[tokio::test]
async fn the_host_header_and_upstream_url_use_the_chosen_endpoint() {
    let upstreams = backends(2).await;
    let proxy = proxy_config(&upstreams, policy(LbAlgorithm::RoundRobin), |site| {
        site.routes[0].preserve_host = false;
    })
    .await;
    for _ in 0..4 {
        get(proxy.addr, HOST, "/", "").await;
    }
    for upstream in &upstreams {
        let recorded = upstream.requests.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        let expected = upstream.addr.to_string();
        assert!(recorded
            .iter()
            .all(|r| r.headers.get("host").unwrap() == expected.as_str()));
    }
}

#[tokio::test]
async fn draining_and_disabled_endpoints_get_no_new_requests() {
    let upstreams = backends(3).await;
    let proxy = proxy_config(&upstreams, policy(LbAlgorithm::RoundRobin), |site| {
        let endpoints = &mut site.upstream_groups[0].endpoints;
        endpoints[0].state = EndpointState::Draining;
        endpoints[1].state = EndpointState::Disabled;
    })
    .await;
    for _ in 0..9 {
        get(proxy.addr, HOST, "/", "").await;
    }
    assert_eq!(hits(&upstreams), [0, 0, 9]);
}

fn mark_all_unhealthy(proxy: &Harness) {
    let snapshot = proxy.holder.load();
    for subsystem in snapshot.subsystems.values() {
        for upstream in subsystem.upstreams.values() {
            for candidate in &upstream.candidates {
                candidate.runtime.record_probe(false, 1, 1);
            }
        }
    }
}

#[tokio::test]
async fn fail_open_keeps_serving_when_every_endpoint_is_unhealthy() {
    let upstreams = backends(2).await;
    let proxy = proxy(&upstreams, policy(LbAlgorithm::RoundRobin)).await;
    mark_all_unhealthy(&proxy);
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    assert_eq!(hits(&upstreams).iter().sum::<usize>(), 1);
}

#[tokio::test]
async fn fail_closed_answers_503_when_every_endpoint_is_unhealthy() {
    let upstreams = backends(2).await;
    let mut closed = policy(LbAlgorithm::RoundRobin);
    closed.failure = FailurePolicy::FailClosed;
    let proxy = proxy(&upstreams, closed).await;
    mark_all_unhealthy(&proxy);
    let reply = get(proxy.addr, HOST, "/", "Accept: application/json\r\n").await;
    assert_eq!(reply.status, 503);
    assert!(reply.body.contains("no_healthy_upstream"), "{}", reply.body);
    assert_eq!(hits(&upstreams), [0, 0]);
    assert_eq!(upstreams[0].connections.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn all_endpoints_draining_is_a_503_even_when_failing_open() {
    let upstreams = backends(2).await;
    let proxy = proxy_config(&upstreams, policy(LbAlgorithm::RoundRobin), |site| {
        for endpoint in &mut site.upstream_groups[0].endpoints {
            endpoint.state = EndpointState::Draining;
        }
    })
    .await;
    let reply = get(proxy.addr, HOST, "/", "").await;
    assert_eq!(reply.status, 503);
    assert_eq!(hits(&upstreams), [0, 0]);
}
