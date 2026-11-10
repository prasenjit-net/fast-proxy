//! The M2 exit criteria, end to end: the real proxy in front of three
//! backends that can be killed, slowed and made to fail on demand.
//!
//! 1. Traffic is spread across a group's endpoints by its algorithm.
//! 2. Killing a backend under load causes no client errors, and the endpoint
//!    leaves rotation within a bounded time.
//! 3. A revived endpoint returns by itself.
//! 4. Draining or disabling an endpoint stops new requests, lets in-flight
//!    ones finish, and survives a restart.
//! 5. With no healthy endpoint a group fails open or answers 503 as it says.
//!
//! Detection is shown both ways: passively (failures seen by the proxy) and
//! actively (health probes, with no client traffic at all).

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use common::flaky::FlakyBackend;
use common::groups::*;
use common::*;
use serde_json::{json, Value};

const HOST: &str = "shop.example.com";

fn three_backends() -> Vec<FlakyBackend> {
    ["A", "B", "C"]
        .into_iter()
        .map(FlakyBackend::start)
        .collect()
}

fn ports(backends: &[FlakyBackend]) -> Vec<u16> {
    backends.iter().map(|b| b.port).collect()
}

fn hits(backends: &[FlakyBackend]) -> Vec<usize> {
    backends.iter().map(FlakyBackend::hits).collect()
}

fn get(proxy: &Proxy, path: &str) -> (u16, String) {
    through_proxy(proxy, HOST, path)
}

/// Active checks quick enough for a test but tolerant of a loaded machine:
/// the tests run in parallel, and a probe that times out spuriously would
/// flap a healthy endpoint (the group then rightly falls back to panic mode).
fn fast_checks() -> Value {
    json!({ "health": {
        "check": "http", "path": "/healthz",
        "intervalMs": 500, "timeoutMs": 500, "rise": 2, "fall": 2
    } })
}

fn site(proxy: &Proxy, backends: &[FlakyBackend], policy: Value) -> (i64, i64) {
    create_group_site(proxy, "shop", HOST, &ports(backends), policy)
}

fn endpoint_metric(proxy: &Proxy, name: &str, port: u16, outcome: Option<&str>) -> f64 {
    let outcome = outcome.map_or(String::new(), |o| format!(r#"outcome="{o}","#));
    let labels =
        format!(r#"{{endpoint="127.0.0.1:{port}",{outcome}subsystem="shop",upstream="backend"}}"#);
    metric(proxy, &format!("{name}{labels}")).unwrap_or(0.0)
}

/// Failed attempts on the endpoint, of any kind: a request already in flight
/// when a backend dies can fail as a broken connection rather than a refused one.
fn failed_attempts(proxy: &Proxy, port: u16) -> f64 {
    ["connect", "other", "timeout", "bad_status"]
        .into_iter()
        .map(|outcome| {
            endpoint_metric(
                proxy,
                "fastproxy_upstream_requests_total",
                port,
                Some(outcome),
            )
        })
        .sum()
}

/// Sends requests (about 50 a second) until `condition` holds or `seconds` pass.
fn drive_until(proxy: &Proxy, seconds: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        get(proxy, "/");
        thread::sleep(Duration::from_millis(20));
    }
    false
}

// ---- criterion 1: distribution ------------------------------------------

#[test]
fn round_robin_spreads_requests_exactly_evenly_over_three_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    site(&proxy, &backends, json!({}));
    for _ in 0..30 {
        assert_eq!(get(&proxy, "/").0, 200);
    }
    assert_eq!(hits(&backends), [10, 10, 10]);
}

#[test]
fn weighted_round_robin_splits_traffic_by_weight() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(
        &proxy,
        &backends,
        json!({ "lb": { "algorithm": "weighted_round_robin" } }),
    );
    set_weight(&proxy, sub, up, backends[0].port, 3);
    for _ in 0..50 {
        assert_eq!(get(&proxy, "/").0, 200);
    }
    assert_eq!(hits(&backends), [30, 10, 10]);
}

#[test]
fn consistent_hash_keeps_a_key_on_one_endpoint_and_spreads_different_keys() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    site(
        &proxy,
        &backends,
        json!({ "lb": { "algorithm": "consistent_hash", "hashKey": "header:x-user" } }),
    );
    let answers: Vec<String> = (0..10)
        .map(|_| get_with_headers(&proxy, HOST, "/", &["x-user: alice"]).1)
        .collect();
    assert!(
        answers.iter().all(|a| a == &answers[0]),
        "one key, one endpoint: {answers:?}"
    );
    for user in 0..60 {
        get_with_headers(&proxy, HOST, "/", &[&format!("x-user: user{user}")]);
    }
    assert!(
        hits(&backends).iter().all(|h| *h > 0),
        "{:?}",
        hits(&backends)
    );
}

#[test]
fn least_connections_and_lowest_latency_serve_every_request_across_all_endpoints() {
    for algorithm in ["least_connections", "p2c_ewma"] {
        let dir = tempfile::tempdir().unwrap();
        let proxy = start_proxy(dir.path());
        let backends = three_backends();
        site(
            &proxy,
            &backends,
            json!({ "lb": { "algorithm": algorithm } }),
        );
        for _ in 0..60 {
            assert_eq!(get(&proxy, "/").0, 200, "{algorithm}");
        }
        let counts = hits(&backends);
        assert_eq!(counts.iter().sum::<usize>(), 60);
        assert!(counts.iter().all(|h| *h > 0), "{algorithm}: {counts:?}");
    }
}

// ---- criteria 2 and 3: failure and recovery ------------------------------

#[test]
fn killing_one_of_three_backends_under_load_causes_no_client_errors_and_it_leaves_rotation() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(&proxy, &backends, json!({}));
    let victim = &backends[1];

    let stop = Arc::new(AtomicBool::new(false));
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let clients: Vec<_> = (0..4)
        .map(|_| {
            let (addr, stop, statuses) = (proxy.data, stop.clone(), statuses.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (status, _) = http(addr, "GET", "/", HOST, None);
                    statuses.lock().unwrap().push(status);
                }
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(300));

    victim.kill();
    let killed_at = Instant::now();
    assert!(
        wait_within(20, || effective_state(&proxy, sub, up, victim.port)
            == "ejected"),
        "the dead endpoint should be ejected after three consecutive failures"
    );
    let detection = killed_at.elapsed();
    let attempts_at_detection = failed_attempts(&proxy, victim.port);
    thread::sleep(Duration::from_secs(1));
    stop.store(true, Ordering::Relaxed);
    for client in clients {
        client.join().unwrap();
    }

    let statuses = statuses.lock().unwrap();
    assert!(statuses.len() > 50, "only {} requests ran", statuses.len());
    let failures: Vec<_> = statuses.iter().filter(|s| **s != 200).collect();
    assert!(
        failures.is_empty(),
        "{} of {} requests failed: {failures:?}",
        failures.len(),
        statuses.len()
    );

    // Once ejected the proxy stops trying it: at most the attempts that were
    // already under way when the verdict came in.
    let attempts_at_end = failed_attempts(&proxy, victim.port);
    assert!(attempts_at_detection >= 3.0, "the strikes were counted");
    assert!(
        attempts_at_end - attempts_at_detection <= 4.0,
        "{attempts_at_detection} -> {attempts_at_end}"
    );
    assert_eq!(
        endpoint_metric(
            &proxy,
            "fastproxy_endpoint_ejections_total",
            victim.port,
            None
        ),
        1.0
    );
    println!(
        "{} requests, no errors; the dead endpoint left rotation {detection:?} after the kill",
        statuses.len()
    );
}

#[test]
fn a_revived_backend_returns_to_rotation_by_itself_after_its_ejection_expires() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let policy = json!({ "passive": {
        "consecutiveFailures": 2, "baseEjectionMs": 500, "maxEjectionMs": 500, "maxEjectionPercent": 50
    } });
    let (sub, up) = site(&proxy, &backends, policy);
    let victim = &backends[1];

    victim.kill();
    assert!(
        drive_until(&proxy, 20, || effective_state(&proxy, sub, up, victim.port)
            == "ejected"),
        "ejected after two failures"
    );
    victim.revive();
    let before = victim.hits();
    assert!(
        drive_until(&proxy, 20, || victim.hits() > before
            && effective_state(&proxy, sub, up, victim.port) == "healthy"),
        "the revived endpoint should serve again with no operator action"
    );
}

#[test]
fn active_checks_mark_an_endpoint_unhealthy_and_healthy_again_with_no_client_traffic() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(&proxy, &backends, fast_checks());
    let sick = &backends[1];
    assert!(wait_within(10, || sick.probes() >= 2), "probes are running");

    sick.set_health_status(500);
    assert!(
        wait_within(10, || effective_state(&proxy, sub, up, sick.port)
            == "unhealthy"),
        "marked unhealthy after `fall` failed probes"
    );
    assert_eq!(
        sick.hits(),
        0,
        "found by probes alone: no client request ever reached it"
    );
    for _ in 0..30 {
        assert_eq!(get(&proxy, "/").0, 200);
    }
    assert_eq!(sick.hits(), 0, "an unhealthy endpoint receives no requests");

    sick.set_health_status(0);
    assert!(
        wait_within(10, || effective_state(&proxy, sub, up, sick.port)
            == "healthy"),
        "healthy again after `rise` good probes"
    );
    for _ in 0..9 {
        get(&proxy, "/");
    }
    assert!(sick.hits() > 0, "and it takes traffic again");
}

// ---- criterion 4: drain and disable --------------------------------------

#[test]
fn draining_stops_new_requests_while_the_one_in_flight_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(&proxy, &backends, json!({}));
    backends[0].set_delay(Duration::from_millis(800));
    let endpoint = endpoint_on(&proxy, sub, up, backends[0].port);

    // Round robin starts at the first endpoint, which is the slow one.
    let addr = proxy.data;
    let in_flight = thread::spawn(move || http(addr, "GET", "/slow", HOST, None));
    assert!(
        wait_within(10, || backends[0].hits() == 1),
        "the slow request is on endpoint A"
    );
    set_endpoint_state(&proxy, sub, up, endpoint, "draining");
    assert_eq!(
        effective_state(&proxy, sub, up, backends[0].port),
        "draining"
    );

    for _ in 0..12 {
        let (status, body) = get(&proxy, "/");
        assert_eq!(status, 200);
        assert!(
            !body.starts_with('A'),
            "a new request reached the draining endpoint: {body}"
        );
    }
    assert_eq!(backends[0].hits(), 1);
    let (status, body) = in_flight.join().unwrap();
    assert_eq!(
        (status, body.as_str()),
        (200, "A /slow"),
        "the in-flight request finished"
    );

    let drained = wait_within(10, || {
        let status = upstream_status(&proxy, sub, up);
        status["endpoints"][0]["inFlight"] == 0
    });
    assert!(drained, "nothing is left on the drained endpoint");
}

#[test]
fn disabling_takes_an_endpoint_out_enabling_brings_it_back_and_the_state_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(&proxy, &backends, json!({}));
    let endpoint = endpoint_on(&proxy, sub, up, backends[2].port);

    set_endpoint_state(&proxy, sub, up, endpoint, "disabled");
    for _ in 0..12 {
        assert_eq!(get(&proxy, "/").0, 200);
    }
    assert_eq!(backends[2].hits(), 0, "a disabled endpoint gets nothing");

    // Restart on the same ports and the same database.
    let (data, admin_addr) = (proxy.data.port(), proxy.admin.port());
    drop(proxy);
    let proxy = start_proxy_on(dir.path(), data, admin_addr);
    assert_eq!(
        effective_state(&proxy, sub, up, backends[2].port),
        "disabled"
    );
    for _ in 0..12 {
        get(&proxy, "/");
    }
    assert_eq!(backends[2].hits(), 0, "still disabled after the restart");

    set_endpoint_state(&proxy, sub, up, endpoint, "enabled");
    for _ in 0..12 {
        get(&proxy, "/");
    }
    assert!(backends[2].hits() > 0, "enabled again");
}

// ---- criterion 5: no healthy endpoint --------------------------------------

#[test]
fn with_no_healthy_endpoint_a_group_fails_open_or_closed_as_configured() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (open_sub, open_up) = site(&proxy, &backends, fast_checks());
    let mut closed_policy = fast_checks();
    closed_policy["failurePolicy"] = json!("fail_closed");
    let (closed_sub, closed_up) = create_group_site(
        &proxy,
        "closed",
        "closed.example.com",
        &ports(&backends),
        closed_policy,
    );

    for backend in &backends {
        backend.set_health_status(500);
    }
    let none_healthy = |sub, up| upstream_status(&proxy, sub, up)["healthy"] == 0;
    assert!(
        wait_within(15, || none_healthy(open_sub, open_up)
            && none_healthy(closed_sub, closed_up)),
        "every endpoint of both groups is unhealthy"
    );

    // Failing open ("panic mode") keeps serving.
    assert_eq!(get(&proxy, "/").0, 200);
    assert!(
        metric(
            &proxy,
            r#"fastproxy_upstream_panic_total{subsystem="shop",upstream="backend"}"#
        )
        .unwrap_or(0.0)
            >= 1.0
    );
    // Failing closed answers 503.
    let (status, body) = through_proxy(&proxy, "closed.example.com", "/");
    assert_eq!(status, 503);
    assert!(body.contains("no healthy upstream"), "{body}");

    for backend in &backends {
        backend.set_health_status(0);
    }
    assert!(
        wait_within(15, || !none_healthy(closed_sub, closed_up)),
        "the group recovers by itself"
    );
    assert_eq!(through_proxy(&proxy, "closed.example.com", "/").0, 200);
}

#[test]
fn a_group_whose_endpoints_are_all_draining_answers_503_even_when_failing_open() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backends = three_backends();
    let (sub, up) = site(&proxy, &backends, json!({}));
    for (endpoint, _) in endpoints_of(&proxy, sub, up) {
        set_endpoint_state(&proxy, sub, up, endpoint, "draining");
    }
    let (status, body) = get(&proxy, "/");
    assert_eq!(status, 503);
    assert!(body.contains("no healthy upstream"), "{body}");
    assert_eq!(hits(&backends), [0, 0, 0]);
}
