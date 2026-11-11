//! Maintenance toggled under traffic: clients keep sending requests through the
//! real proxy while an admin task turns maintenance on and off for each scope,
//! again and again, and lets a TTL expire. No request may fail except with the
//! maintenance 503; a request that ran entirely after a toggle returned and
//! before the next one began must see exactly that state (the sharp rule of
//! `reload_under_load`); and the backend receives exactly the requests that were
//! answered 200, so nothing in maintenance ever reached it.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::flaky::FlakyBackend;
use common::groups::create_group_site;
use common::load::{client_loop, Sample};
use common::*;
use serde_json::{json, Value};

const HOST: &str = "shop.example.com";
const CLIENTS: usize = 8;
const ROUNDS_PER_SCOPE: usize = 4;
const MESSAGE: &str = "under-load-maintenance";
const BACKEND: &str = "shop-backend";

/// What a scope closes, for the sharp rule.
#[derive(Clone, Copy, PartialEq)]
enum Scope {
    Global,
    Subsystem,
    /// Only requests under `/api`.
    Route,
}

impl Scope {
    fn covers(self, path: &str) -> bool {
        match self {
            Scope::Global | Scope::Subsystem => true,
            Scope::Route => path.starts_with("/api"),
        }
    }
}

/// One turn on and off: the write was sent at `.before` and had returned by `.after`.
struct Round {
    scope: Scope,
    on_before: Instant,
    on_after: Instant,
    off_before: Instant,
    off_after: Instant,
}

fn put_and_delete_paths(subsystem: i64, route: i64, scope: Scope) -> String {
    match scope {
        Scope::Global => "/api/maintenance/global".to_string(),
        Scope::Subsystem => format!("/api/subsystems/{subsystem}/maintenance"),
        Scope::Route => format!("/api/subsystems/{subsystem}/routes/{route}/maintenance"),
    }
}

#[test]
fn maintenance_toggled_under_load_fails_no_request_and_never_reaches_the_backend() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = FlakyBackend::start(BACKEND);
    // A short delay keeps requests in flight while maintenance is toggled.
    backend.set_delay(Duration::from_millis(5));
    let (subsystem, upstream) = create_group_site(&proxy, "shop", HOST, &[backend.port], json!({}));
    let (status, reply) = admin(
        &proxy,
        "POST",
        &format!("/api/subsystems/{subsystem}/routes"),
        Some(json!({ "prefix": "/api", "upstreamGroupId": upstream })),
    );
    assert_eq!(status, 201, "{reply}");
    let api_route = reply["data"]["id"].as_i64().unwrap();
    assert_eq!(through_proxy(&proxy, HOST, "/api/x").0, 200);
    let baseline_hits = backend.hits();

    let stop = Arc::new(AtomicBool::new(false));
    let clients: Vec<_> = (0..CLIENTS)
        .map(|n| {
            let path = if n % 2 == 0 { "/" } else { "/api/x" };
            let (addr, stop) = (proxy.data, stop.clone());
            thread::spawn(move || client_loop(addr, HOST, path, stop))
        })
        .collect();
    thread::sleep(Duration::from_millis(100)); // let the load build up first
    let origin = Instant::now();

    let on_body = json!({ "message": MESSAGE });
    let mut rounds: Vec<Round> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for _ in 0..ROUNDS_PER_SCOPE {
        for scope in [Scope::Global, Scope::Subsystem, Scope::Route] {
            let path = put_and_delete_paths(subsystem, api_route, scope);
            let on_before = Instant::now();
            let (status, reply) = admin(&proxy, "PUT", &path, Some(on_body.clone()));
            let on_after = Instant::now();
            if status != 200 {
                errors.push(format!("PUT {path} -> {status} {reply}"));
            }
            thread::sleep(Duration::from_millis(60));
            let off_before = Instant::now();
            let (status, reply) = admin(&proxy, "DELETE", &path, None);
            let off_after = Instant::now();
            if status != 200 {
                errors.push(format!("DELETE {path} -> {status} {reply}"));
            }
            thread::sleep(Duration::from_millis(40));
            rounds.push(Round {
                scope,
                on_before,
                on_after,
                off_before,
                off_after,
            });
        }
    }

    // A TTL that ends by itself, under the same traffic: the end is observed
    // through the traffic, not slept for. Requests during it are unconstrained
    // by the sharp rule but still bound by the no-failure rule.
    let ttl_on = Instant::now();
    let (status, _) = admin(
        &proxy,
        "PUT",
        "/api/maintenance/global",
        Some(json!({ "message": MESSAGE, "ttlSecs": 1 })),
    );
    assert_eq!(status, 200);
    // The probe that sees service resume is itself a request the backend served.
    let mut probe_served = 0usize;
    assert!(
        wait_until(|| {
            let answered = through_proxy(&proxy, HOST, "/").0 == 200;
            probe_served += usize::from(answered);
            answered
        }),
        "the TTL never ended maintenance"
    );
    let ttl_seen_over = Instant::now();
    assert!(
        wait_until(|| admin(&proxy, "GET", "/api/maintenance", None).1["data"] == json!([])),
        "the sweeper never cleared the expired scope"
    );

    thread::sleep(Duration::from_millis(100));
    stop.store(true, Ordering::Relaxed);
    let samples: Vec<Sample> = clients
        .into_iter()
        .flat_map(|client| client.join().unwrap())
        .collect();
    let end = Instant::now();

    // 1. The admin side accepted every toggle.
    assert!(errors.is_empty(), "admin writes failed: {errors:?}");

    // 2. Real load happened, through real toggles.
    assert!(
        samples.len() > 500,
        "only {} requests were made",
        samples.len()
    );

    // 3. Nothing failed except with the maintenance page; and a 200 is the backend's.
    let mut served = 0usize;
    let mut closed = 0usize;
    let mut bad: Vec<String> = Vec::new();
    for sample in &samples {
        match &sample.outcome {
            Ok((200, body)) if body.starts_with(BACKEND) => served += 1,
            Ok((503, body)) if body.contains(MESSAGE) => closed += 1,
            Ok((status, body)) => bad.push(format!("{} -> {status} {body:?}", sample.path)),
            Err(error) => bad.push(format!("{} -> {error}", sample.path)),
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} requests failed, e.g. {:?}",
        bad.len(),
        samples.len(),
        &bad[..bad.len().min(5)]
    );
    assert!(
        served > 100 && closed > 100,
        "served {served}, closed {closed}"
    );

    // 4. The sharp rule. A request that ran entirely between two toggles saw
    //    exactly the state the first of them set. A path a scope does not cover
    //    is served throughout that scope's round.
    let state_of = |sample: &Sample| -> Option<u16> {
        for (i, round) in rounds.iter().enumerate() {
            let next_on = rounds.get(i + 1).map_or(ttl_on, |next| next.on_before);
            if sample.started >= round.on_after && sample.finished <= round.off_before {
                return Some(if round.scope.covers(sample.path) {
                    503
                } else {
                    200
                });
            }
            if sample.started >= round.off_after && sample.finished <= next_on {
                return Some(200);
            }
        }
        // Before the first toggle, and after the TTL was seen to end.
        if sample.finished <= rounds[0].on_before && sample.started >= origin {
            return Some(200);
        }
        (sample.started >= ttl_seen_over && sample.finished <= end).then_some(200)
    };
    let (mut checked_closed, mut checked_open) = (0usize, 0usize);
    for sample in &samples {
        let (status, _) = sample.outcome.as_ref().unwrap();
        if let Some(expected) = state_of(sample) {
            assert_eq!(
                *status, expected,
                "{} ran between two toggles and saw the wrong state",
                sample.path
            );
            if expected == 503 {
                checked_closed += 1;
            } else {
                checked_open += 1;
            }
        }
    }
    assert!(
        checked_closed > 50 && checked_open > 50,
        "only {checked_closed} closed and {checked_open} open requests fell inside a stable window"
    );

    // 5. The backend got exactly what was answered 200: nothing in maintenance reached it.
    assert_eq!(
        backend.hits() - baseline_hits,
        served + probe_served,
        "the backend's requests and the 200 answers differ: a maintenance answer reached it"
    );

    // 6. The proxy's own accounting agrees: maintenance answers are counted apart, and
    //    the live state ends open.
    let (_, metrics) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    let counted: f64 = metrics
        .lines()
        .filter(|line| line.starts_with("fastproxy_maintenance_responses_total{"))
        .filter_map(|line| line.rsplit(' ').next()?.parse::<f64>().ok())
        .sum();
    assert!(
        counted >= closed as f64,
        "the proxy counted {counted} maintenance answers, the clients saw {closed}"
    );
    assert_eq!(through_proxy(&proxy, HOST, "/").0, 200);
    let (_, list): (u16, Value) = admin(&proxy, "GET", "/api/maintenance", None);
    assert_eq!(list["data"], json!([]));
    println!(
        "{} requests on {CLIENTS} keep-alive connections during {} toggles and a TTL expiry; \
         {checked_closed} closed and {checked_open} open requests ran entirely between two toggles; \
         {served} served, {closed} answered with the maintenance page",
        samples.len(),
        rounds.len() * 2
    );
}
