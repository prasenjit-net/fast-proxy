//! Live configuration edits under traffic: many clients keep sending requests
//! through the real proxy while an admin task makes hundreds of writes. No
//! request may fail, and every answer must come from a backend that was
//! valid for that route at some point while the request was in flight.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::groups::{create_group_site, endpoint_on};
use common::load::{client_loop, Sample};
use common::*;
use serde_json::{json, Value};

const CLIENTS: usize = 8;
const WRITES: usize = 400;
const HOST: &str = "shop.example.com";

/// When a `/flip` target was written: it became live at some point between
/// `before` (the write was sent) and `after` (the response came back).
struct Flip {
    before: Instant,
    after: Instant,
    target: &'static str,
}

struct Admin<'a> {
    proxy: &'a Proxy,
    errors: Vec<String>,
}

impl Admin<'_> {
    /// Sends one write; anything but a 2xx is recorded as an error.
    fn write(&mut self, method: &str, path: &str, body: Option<Value>) -> Value {
        let (status, reply) = admin(self.proxy, method, path, body);
        if !(200..300).contains(&status) {
            self.errors
                .push(format!("{method} {path} -> {status}: {reply}"));
        }
        reply
    }
}

#[test]
fn live_config_edits_under_load_drop_no_requests_and_serve_only_valid_backends() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    // A short delay keeps requests in flight while the configuration changes.
    let backend_a = start_keepalive_backend("A", Duration::from_millis(5));
    let backend_b = start_keepalive_backend("B", Duration::from_millis(5));

    // shop: "/" always goes to A; "/flip" starts on A and is flipped between A and B.
    let (id, upstream_a) = create_site(&proxy, "shop", HOST, backend_a);
    let base = format!("/api/subsystems/{id}");
    let (status, reply) = admin(
        &proxy,
        "POST",
        &format!("{base}/upstreams"),
        Some(json!({
            "name": "b",
            "endpoints": [{ "addr": format!("127.0.0.1:{backend_b}") }]
        })),
    );
    assert_eq!(status, 201, "{reply}");
    let upstream_b = reply["data"]["id"].as_i64().unwrap();
    let route_body =
        |prefix: &str, upstream: i64| json!({ "prefix": prefix, "upstreamGroupId": upstream });
    let (status, reply) = admin(
        &proxy,
        "POST",
        &format!("{base}/routes"),
        Some(route_body("/flip", upstream_a)),
    );
    assert_eq!(status, 201, "{reply}");
    let flip_route = reply["data"]["id"].as_i64().unwrap();
    let setup_version = admin(&proxy, "GET", "/api/status", None).1["version"]
        .as_u64()
        .unwrap();

    // The initial state (everything on A) holds from here on, before any traffic.
    let origin = Instant::now();

    // Traffic: half the clients hit the stable route, half the flipping one.
    let stop = Arc::new(AtomicBool::new(false));
    let clients: Vec<_> = (0..CLIENTS)
        .map(|n| {
            let path = if n % 2 == 0 { "/" } else { "/flip" };
            let (addr, stop) = (proxy.data, stop.clone());
            thread::spawn(move || client_loop(addr, HOST, path, stop))
        })
        .collect();
    thread::sleep(Duration::from_millis(100)); // let the load build up first

    // Admin: hundreds of writes, one successful write per iteration.
    let mut flips = vec![Flip {
        before: origin,
        after: origin,
        target: "A",
    }];
    let mut admin_calls = Admin {
        proxy: &proxy,
        errors: Vec::new(),
    };
    let (mut temp_route, mut extra_subsystem) = (0_i64, 0_i64);
    let mut writes_made = 0_u64;
    for i in 0..WRITES {
        match i % 5 {
            0 => {
                let (target, upstream) = if flips.last().map(|f| f.target) == Some("A") {
                    ("B", upstream_b)
                } else {
                    ("A", upstream_a)
                };
                let before = Instant::now();
                admin_calls.write(
                    "PUT",
                    &format!("{base}/routes/{flip_route}"),
                    Some(route_body("/flip", upstream)),
                );
                flips.push(Flip {
                    before,
                    after: Instant::now(),
                    target,
                });
            }
            1 => {
                let reply = admin_calls.write(
                    "POST",
                    &format!("{base}/routes"),
                    Some(route_body(&format!("/tmp{i}"), upstream_a)),
                );
                temp_route = reply["data"]["id"].as_i64().unwrap_or(0);
            }
            2 => {
                admin_calls.write("DELETE", &format!("{base}/routes/{temp_route}"), None);
            }
            3 => {
                let reply = admin_calls.write(
                    "POST",
                    "/api/subsystems",
                    Some(json!({ "name": format!("extra{i}") })),
                );
                extra_subsystem = reply["data"]["id"].as_i64().unwrap_or(0);
            }
            _ => {
                admin_calls.write(
                    "DELETE",
                    &format!("/api/subsystems/{extra_subsystem}"),
                    None,
                );
            }
        }
        writes_made += 1;
        thread::sleep(Duration::from_millis(3));
    }
    thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    let samples: Vec<Sample> = clients
        .into_iter()
        .flat_map(|c| c.join().unwrap())
        .collect();

    // 1. The admin side accepted every write.
    assert!(
        admin_calls.errors.is_empty(),
        "admin writes failed: {:?}",
        admin_calls.errors
    );

    // 2. Real load happened.
    // Enough to be real load even on a small CI runner (a laptop makes ~2,800).
    assert!(
        samples.len() > 500,
        "only {} requests were made",
        samples.len()
    );

    // 3. No request failed, and "/" (never edited) always reached A.
    let failures: Vec<String> = samples
        .iter()
        .filter_map(|s| match &s.outcome {
            Ok((200, _)) => None,
            Ok((status, body)) => Some(format!("{} -> {status} {body:?}", s.path)),
            Err(error) => Some(format!("{} -> {error}", s.path)),
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} requests failed, e.g. {:?}",
        failures.len(),
        samples.len(),
        &failures[..failures.len().min(5)]
    );
    for sample in samples.iter().filter(|s| s.path == "/") {
        assert_eq!(
            sample.outcome.as_ref().unwrap().1,
            "A",
            "the unedited route changed backend"
        );
    }

    // 4. Every "/flip" answer came from a backend that route pointed at while
    //    the request was in flight. A target is live from the moment its write
    //    was sent until the next write has certainly returned.
    let valid_during = |sample: &Sample, body: &str| {
        flips.iter().enumerate().any(|(i, flip)| {
            let live_until = flips.get(i + 1).map_or(sample.finished, |next| next.after);
            flip.target == body && flip.before <= sample.finished && live_until >= sample.started
        })
    };
    // 5. A request that ran entirely between two writes must see exactly the
    //    value written before it: a stale snapshot would show up here.
    let mut seen = std::collections::BTreeSet::new();
    let mut stable_checked = 0;
    for sample in samples.iter().filter(|s| s.path == "/flip") {
        let body = &sample.outcome.as_ref().unwrap().1;
        seen.insert(body.clone());
        assert!(
            valid_during(sample, body),
            "/flip answered {body:?} at a time it never pointed there"
        );
        for (i, flip) in flips.iter().enumerate() {
            let window_end = flips.get(i + 1).map_or(sample.finished, |next| next.before);
            if sample.started >= flip.after && sample.finished <= window_end {
                assert_eq!(
                    body, flip.target,
                    "a request between two writes saw a stale route"
                );
                stable_checked += 1;
            }
        }
    }
    assert!(
        seen.contains("A") && seen.contains("B"),
        "the flip never took effect under load: {seen:?}"
    );
    assert!(
        stable_checked > 100,
        "only {stable_checked} requests fell inside a stable window"
    );

    // 6. The proxy's own accounting agrees: no 5xx, no rejected change, and
    //    one config version per write.
    let (_, metrics) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    assert!(
        !metrics.contains("status_class=\"5xx\""),
        "the proxy counted 5xx responses"
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_config_reloads_total{outcome="rejected"}"#
        ),
        None
    );
    assert_eq!(
        metric(&proxy, "fastproxy_config_version"),
        Some((setup_version + writes_made) as f64)
    );

    println!(
        "{} requests on {CLIENTS} keep-alive connections during {writes_made} config writes ({} flips); \
         {stable_checked} requests ran entirely between two writes; 0 failures",
        samples.len(),
        flips.len() - 1
    );

    // 7. After the dust settles the live state is the last thing written.
    let last = flips.last().map(|f| f.target).unwrap_or("A");
    assert_eq!(
        through_proxy(&proxy, HOST, "/flip"),
        (200, last.to_string())
    );
}

/// A span during which an endpoint must receive no new request: from the
/// moment the write that drained, disabled or removed it returned, until the
/// next write that brings it back is sent.
struct Closed {
    backend: &'static str,
    from: Instant,
    until: Instant,
}

#[test]
fn endpoint_changes_under_load_drop_no_requests_and_a_closed_endpoint_gets_nothing_new() {
    const NAMES: [&str; 4] = ["A", "B", "C", "D"];
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let ports: Vec<u16> = NAMES
        .iter()
        .map(|name| start_keepalive_backend(name, Duration::from_millis(5)))
        .collect();
    let (id, upstream) = create_group_site(&proxy, "shop", HOST, &ports[..3], json!({}));
    let endpoints = format!("/api/subsystems/{id}/upstreams/{upstream}/endpoints");
    let setup_version = admin(&proxy, "GET", "/api/status", None).1["version"]
        .as_u64()
        .unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let clients: Vec<_> = (0..CLIENTS)
        .map(|_| {
            let (addr, stop) = (proxy.data, stop.clone());
            thread::spawn(move || client_loop(addr, HOST, "/", stop))
        })
        .collect();
    thread::sleep(Duration::from_millis(100));

    // Admin: drain and re-enable B, disable and re-enable C, add and remove D.
    let mut errors = Vec::new();
    let mut closed: Vec<Closed> = Vec::new();
    let mut open: std::collections::HashMap<&'static str, Instant> = Default::default();
    let mut writes = 0_u64;
    let mut check = |status: u16, what: &str| {
        if !(200..300).contains(&status) {
            errors.push(format!("{what} -> {status}"));
        }
    };
    for _ in 0..40 {
        let b = endpoint_on(&proxy, id, upstream, ports[1]);
        let c = endpoint_on(&proxy, id, upstream, ports[2]);
        let state = |endpoint: i64, state: &str| {
            let (status, _) = admin(
                &proxy,
                "PUT",
                &format!("{endpoints}/{endpoint}/state"),
                Some(json!({ "state": state })),
            );
            status
        };

        // Take B out (drain), then bring it back.
        check(state(b, "draining"), "drain B");
        open.insert("B", Instant::now());
        thread::sleep(Duration::from_millis(15));
        let reopened = Instant::now();
        check(state(b, "enabled"), "enable B");
        closed.extend(open.remove("B").map(|from| Closed {
            backend: "B",
            from,
            until: reopened,
        }));

        // Take C out (disable), then bring it back.
        check(state(c, "disabled"), "disable C");
        open.insert("C", Instant::now());
        thread::sleep(Duration::from_millis(15));
        let reopened = Instant::now();
        check(state(c, "enabled"), "enable C");
        closed.extend(open.remove("C").map(|from| Closed {
            backend: "C",
            from,
            until: reopened,
        }));

        // Add D, let it serve a moment, then remove it.
        let (status, reply) = admin(
            &proxy,
            "POST",
            &endpoints,
            Some(json!({ "addr": format!("127.0.0.1:{}", ports[3]) })),
        );
        check(status, "add D");
        let endpoint_d = reply["data"]["id"].as_i64().unwrap_or(0);
        thread::sleep(Duration::from_millis(15));
        let (status, _) = admin(&proxy, "DELETE", &format!("{endpoints}/{endpoint_d}"), None);
        check(status, "remove D");
        open.insert("D", Instant::now());
        thread::sleep(Duration::from_millis(15));
        // D is added again at the next round; its span ends just before that.
        closed.push(Closed {
            backend: "D",
            from: open.remove("D").unwrap(),
            until: Instant::now(),
        });
        writes += 6;
    }
    thread::sleep(Duration::from_millis(200));
    stop.store(true, Ordering::Relaxed);
    let samples: Vec<Sample> = clients
        .into_iter()
        .flat_map(|c| c.join().unwrap())
        .collect();

    assert!(errors.is_empty(), "admin writes failed: {errors:?}");
    assert!(
        samples.len() > 300,
        "only {} requests were made",
        samples.len()
    );

    // No request failed.
    let failures: Vec<String> = samples
        .iter()
        .filter_map(|s| match &s.outcome {
            Ok((200, _)) => None,
            Ok((status, body)) => Some(format!("{status} {body:?}")),
            Err(error) => Some(error.clone()),
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} requests failed, e.g. {:?}",
        failures.len(),
        samples.len(),
        &failures[..failures.len().min(5)]
    );

    // The sharp rule: a request that started after the write returned never
    // reached the endpoint that write took out.
    let mut checked = 0;
    for span in &closed {
        for sample in samples
            .iter()
            .filter(|s| s.started > span.from && s.started < span.until)
        {
            checked += 1;
            let body = &sample.outcome.as_ref().unwrap().1;
            assert_ne!(
                body, span.backend,
                "a request that began after {} was taken out still reached it",
                span.backend
            );
        }
    }
    assert!(
        checked > 100,
        "only {checked} requests fell inside a closed span"
    );

    // Every backend did serve while it was in: the changes really took effect.
    let seen: std::collections::BTreeSet<String> = samples
        .iter()
        .map(|s| s.outcome.as_ref().unwrap().1.clone())
        .collect();
    assert_eq!(
        seen.len(),
        4,
        "every backend served at some point: {seen:?}"
    );

    // The proxy's own accounting agrees: no 5xx, no rejected change, one
    // config version per write.
    let (_, metrics) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    assert!(
        !metrics.contains("status_class=\"5xx\""),
        "the proxy counted 5xx responses"
    );
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_config_reloads_total{outcome="rejected"}"#
        ),
        None
    );
    assert_eq!(
        metric(&proxy, "fastproxy_config_version"),
        Some((setup_version + writes) as f64)
    );
    println!(
        "{} requests during {writes} endpoint writes; {checked} began inside a span where an endpoint was out, none reached it",
        samples.len()
    );
}
