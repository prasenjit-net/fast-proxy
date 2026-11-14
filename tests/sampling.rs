//! M6 exit criteria for request sampling, against the real binary and real
//! backends: each trigger, the negative case (an upstream's own 5xx), the
//! baseline, overrides, a live settings change, export and clear, the rate limit
//! and the storm guard.

mod common;

use std::time::Duration;

use common::flaky::FlakyBackend;
use common::load::KeepAliveClient;
use common::*;
use serde_json::{json, Value};

const SHOP: &str = "shop.example.com";
const OTHER: &str = "other.example.com";

/// Changes the global sampling settings: reads them, lets `change` edit the
/// writable fields, writes them back. Starts from "no baseline" so a test sees
/// only the trigger it is about.
fn configure(proxy: &Proxy, change: impl FnOnce(&mut Value)) {
    let (status, body) = admin(proxy, "GET", "/api/sampling", None);
    assert_eq!(status, 200, "{body}");
    let mut settings = body["data"]["settings"].clone();
    let fields = settings.as_object_mut().unwrap();
    for derived in ["builtInRedaction", "updatedBy", "updatedAtMs"] {
        fields.remove(derived);
    }
    settings["baselinePerMille"] = json!(0);
    change(&mut settings);
    let (status, body) = admin(proxy, "PUT", "/api/sampling", Some(settings));
    assert_eq!(status, 200, "{body}");
}

fn only_triggers<'a>(names: &'a [&'a str]) -> impl FnOnce(&mut Value) + 'a {
    move |settings| settings["triggers"] = json!(names)
}

fn samples(proxy: &Proxy) -> Vec<Value> {
    let (status, body) = admin(proxy, "GET", "/api/samples?limit=100", None);
    assert_eq!(status, 200, "{body}");
    body["data"].as_array().cloned().unwrap_or_default()
}

fn primaries(proxy: &Proxy) -> Vec<String> {
    samples(proxy)
        .iter()
        .map(|sample| sample["primary"].as_str().unwrap().to_string())
        .collect()
}

/// One sample in full (the list holds only the short columns).
fn detail(proxy: &Proxy, sample: &Value) -> Value {
    let (status, body) = admin(
        proxy,
        "GET",
        &format!("/api/samples/{}", sample["id"]),
        None,
    );
    assert_eq!(status, 200, "{body}");
    body["data"].clone()
}

/// Waits for `count` samples to be stored (batches are written on an interval).
fn await_samples(proxy: &Proxy, count: usize) -> Vec<Value> {
    assert!(
        wait_until(|| samples(proxy).len() >= count),
        "{count} samples were stored in time"
    );
    samples(proxy)
}

/// A settled "nothing was kept": waits for something that was kept and written
/// afterwards (a marker request), so absence is not just "not written yet".
fn assert_nothing_but_marker(proxy: &Proxy, marker_host: &str) {
    assert_eq!(through_proxy(proxy, marker_host, "/marker").0, 502);
    await_samples(proxy, 1);
    let kept = samples(proxy);
    assert_eq!(kept.len(), 1, "only the marker was kept: {kept:?}");
    assert_eq!(detail(proxy, &kept[0])["request"]["path"], "/marker");
}

fn start_with_marker_site(dir: &std::path::Path) -> Proxy {
    let proxy = start_proxy(dir);
    // A site whose backend is dead: its failures are the marker.
    create_site(&proxy, "marker", "marker.example.com", free_port());
    proxy
}

#[test]
fn a_connectivity_failure_is_kept_with_its_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "shop", SHOP, free_port());
    assert_eq!(through_proxy(&proxy, SHOP, "/orders").0, 502);
    let kept = await_samples(&proxy, 1);
    assert_eq!(kept[0]["primary"], "upstream_failure");
    assert_eq!(kept[0]["subsystem"], "shop");
    let (status, detail) = admin(
        &proxy,
        "GET",
        &format!("/api/samples/{}", kept[0]["id"]),
        None,
    );
    assert_eq!(status, 200);
    let attempts = &detail["data"]["upstream"]["attempts"];
    assert_eq!(attempts.as_array().unwrap().len(), 1, "{detail}");
    assert!(attempts[0]["failure"].is_string());
}

#[test]
fn a_connectivity_failure_that_a_retry_recovers_is_kept_as_retried() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let dead = free_port();
    let live = FlakyBackend::start("live");
    let (subsystem, upstream) = create_site(&proxy, "shop", SHOP, live.port);
    let (status, body) = admin(
        &proxy,
        "POST",
        &format!("/api/subsystems/{subsystem}/upstreams/{upstream}/endpoints"),
        Some(json!({ "addr": format!("127.0.0.1:{dead}") })),
    );
    assert_eq!(status, 201, "{body}");
    let mut recovered = false;
    for _ in 0..40 {
        assert_eq!(through_proxy(&proxy, SHOP, "/").0, 200);
        if primaries(&proxy).iter().any(|p| p == "retried") {
            recovered = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        recovered || wait_until(|| primaries(&proxy).iter().any(|p| p == "retried")),
        "a request that failed over to the live endpoint was kept as retried"
    );
}

#[test]
fn a_slow_request_is_kept_and_a_fast_one_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_with_marker_site(dir.path());
    configure(&proxy, |settings| {
        settings["triggers"] = json!(["slow", "upstream_failure"]);
        settings["slowThresholdMs"] = json!(100);
    });
    create_site(
        &proxy,
        "slow",
        SHOP,
        start_slow_backend("slow", Duration::from_millis(400)),
    );
    create_site(&proxy, "fast", OTHER, start_backend("fast"));
    assert_eq!(through_proxy(&proxy, OTHER, "/").0, 200);
    assert_eq!(through_proxy(&proxy, SHOP, "/slow").0, 200);
    let kept = await_samples(&proxy, 1);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0]["primary"], "slow");
    assert_eq!(kept[0]["subsystem"], "slow");
    assert!(kept[0]["totalMs"].as_i64().unwrap() >= 400);
}

#[test]
fn an_upstreams_own_5xx_is_never_sampled() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_with_marker_site(dir.path());
    let backend = FlakyBackend::start("broken");
    backend.fail_with(500);
    create_site(&proxy, "shop", SHOP, backend.port);
    for _ in 0..5 {
        assert_eq!(through_proxy(&proxy, SHOP, "/").0, 500);
    }
    assert_nothing_but_marker(&proxy, "marker.example.com");
}

#[test]
fn the_baseline_keeps_ordinary_requests_when_asked_to() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "shop", SHOP, start_backend("ok"));
    configure(&proxy, |settings| {
        settings["baselinePerMille"] = json!(1000);
        settings["triggers"] = json!(["baseline"]);
    });
    for _ in 0..3 {
        assert_eq!(through_proxy(&proxy, SHOP, "/").0, 200);
    }
    let kept = await_samples(&proxy, 3);
    assert!(kept.iter().all(|sample| sample["primary"] == "baseline"));
}

#[test]
fn an_override_switches_sampling_off_for_one_subsystem_and_back_on() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let (quiet, _) = create_site(&proxy, "quiet", SHOP, free_port());
    create_site(&proxy, "loud", OTHER, free_port());
    configure(&proxy, only_triggers(&["upstream_failure"]));
    let (status, body) = admin(
        &proxy,
        "PUT",
        "/api/sampling/overrides",
        Some(json!({ "subsystemId": quiet, "enabled": false })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(through_proxy(&proxy, SHOP, "/").0, 502);
    assert_eq!(through_proxy(&proxy, OTHER, "/").0, 502);
    let kept = await_samples(&proxy, 1);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0]["subsystem"], "loud");

    let (status, _) = admin(
        &proxy,
        "DELETE",
        &format!("/api/sampling/overrides?subsystemId={quiet}"),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(through_proxy(&proxy, SHOP, "/again").0, 502);
    assert!(wait_until(|| samples(&proxy)
        .iter()
        .any(|sample| sample["subsystem"] == "quiet")));
}

#[test]
fn a_settings_change_applies_at_once_without_dropping_a_connection() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(
        &proxy,
        "shop",
        SHOP,
        start_keepalive_backend("ok", Duration::ZERO),
    );
    configure(&proxy, only_triggers(&["baseline"]));
    let mut client = KeepAliveClient::new(proxy.data, SHOP);
    assert_eq!(client.get("/").unwrap().0, 200);
    assert!(samples(&proxy).is_empty());

    configure(&proxy, |settings| {
        settings["triggers"] = json!(["baseline"]);
        settings["baselinePerMille"] = json!(1000);
    });
    assert_eq!(
        client.get("/same-connection").unwrap().0,
        200,
        "the connection survived"
    );
    let kept = await_samples(&proxy, 1);
    assert_eq!(
        detail(&proxy, &kept[0])["request"]["path"],
        "/same-connection"
    );
}

#[test]
fn samples_can_be_exported_and_cleared() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "shop", SHOP, free_port());
    for _ in 0..2 {
        assert_eq!(through_proxy(&proxy, SHOP, "/").0, 502);
    }
    await_samples(&proxy, 1);
    let (status, text) = http(proxy.admin, "GET", "/api/samples/export", "localhost", None);
    assert_eq!(status, 200);
    let lines: Vec<Value> = text
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(|line| serde_json::from_str(line).expect("one JSON document per line"))
        .collect();
    assert!(!lines.is_empty());
    assert_eq!(lines[0]["primary"], "upstream_failure");

    let (status, body) = admin(&proxy, "DELETE", "/api/samples", None);
    assert_eq!(status, 200, "{body}");
    assert!(body["deleted"].as_i64().unwrap() >= 1);
    assert!(samples(&proxy).is_empty());
    let (_, audit) = admin(&proxy, "GET", "/api/audit?action=samples.clear", None);
    assert!(
        !audit["data"].as_array().unwrap().is_empty(),
        "the clear was audited: {audit}"
    );
}

#[cfg(unix)]
#[test]
fn samples_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (data, admin_port) = (free_port(), free_port());
    let mut proxy = start_proxy_on(dir.path(), data, admin_port);
    create_site(&proxy, "shop", SHOP, free_port());
    assert_eq!(through_proxy(&proxy, SHOP, "/").0, 502);
    await_samples(&proxy, 1);
    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, Duration::from_secs(20)),
        Some(true)
    );
    drop(proxy);

    let proxy = start_proxy_on(dir.path(), data, admin_port);
    assert_eq!(samples(&proxy).len(), 1);
}

#[test]
fn the_rate_limit_caps_what_a_flood_stores_and_counts_what_it_drops() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "shop", SHOP, start_backend("ok"));
    configure(&proxy, |settings| {
        settings["triggers"] = json!(["baseline"]);
        settings["baselinePerMille"] = json!(1000);
        settings["ratePerSec"] = json!(1);
        settings["rateBurst"] = json!(2);
    });
    for _ in 0..50 {
        assert_eq!(through_proxy(&proxy, SHOP, "/").0, 200);
    }
    await_samples(&proxy, 1);
    assert!(wait_until(|| metric(
        &proxy,
        "fastproxy_samples_dropped_total{reason=\"rate_limit\"}"
    )
    .is_some_and(|dropped| dropped >= 30.0)));
    assert!(
        samples(&proxy).len() <= 6,
        "burst plus a few seconds of rate"
    );
}

#[test]
fn the_storm_guard_keeps_a_few_of_a_flood_of_failures_and_counts_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "down", SHOP, free_port());
    create_site(
        &proxy,
        "slow",
        OTHER,
        start_slow_backend("slow", Duration::from_millis(300)),
    );
    configure(&proxy, |settings| {
        settings["triggers"] = json!(["upstream_failure", "slow"]);
        settings["slowThresholdMs"] = json!(100);
        settings["stormBurst"] = json!(3);
        settings["stormIntervalSecs"] = json!(2);
    });

    // A refused connection takes about 2 s to be reported on Windows, so the flood
    // there is smaller; the property (a few kept, the rest counted) is the same.
    let (threads, each) = if cfg!(windows) { (8, 4) } else { (8, 125) };
    let total = threads * each;
    let started = std::time::Instant::now();
    let clients: Vec<_> = (0..threads)
        .map(|_| {
            let addr = proxy.data;
            std::thread::spawn(move || {
                let mut client = KeepAliveClient::new(addr, SHOP);
                for _ in 0..each {
                    assert_eq!(client.get("/x").map(|(status, _)| status), Ok(502));
                }
            })
        })
        .collect();
    for client in clients {
        client.join().unwrap();
    }
    let flood_secs = started.elapsed().as_secs() as usize;
    let mut client = KeepAliveClient::new(proxy.data, SHOP);
    // A request of another kind, during the outage, is still kept.
    assert_eq!(through_proxy(&proxy, OTHER, "/").0, 200);
    assert!(wait_until(|| samples(&proxy)
        .iter()
        .any(|s| s["primary"] == "slow")));
    let failures = |proxy: &Proxy| {
        samples(proxy)
            .into_iter()
            .filter(|s| s["primary"] == "upstream_failure")
            .collect::<Vec<_>>()
    };
    assert!(wait_until(|| failures(&proxy).len() >= 3));
    // 3 for the burst, and one more for each 2 s interval that elapsed during the flood.
    let kept = failures(&proxy).len();
    let allowed = 3 + flood_secs / 2 + 2;
    assert!(
        kept <= allowed,
        "{kept} of {total} failures were kept, at most {allowed} allowed"
    );

    // After the interval the next failure is kept and says how many it stands for.
    std::thread::sleep(Duration::from_millis(2200));
    assert_eq!(client.get("/after").map(|(status, _)| status), Ok(502));
    let stands_for = if cfg!(windows) { 1 } else { total as i64 - 10 };
    assert!(wait_until(|| failures(&proxy).iter().any(|sample| {
        detail(&proxy, sample)["suppressed"]["count"]
            .as_i64()
            .unwrap_or(0)
            >= stands_for
    })));
}
