//! Sampling under load, against the real binary: a flood of connectivity failures
//! on one route while another route serves ordinary traffic. Nothing fails or
//! stalls because of sampling, the counters account for every candidate, the table
//! stays within the storm guard's bound, a configuration write during the flood
//! completes promptly, and shutdown flushes what was queued.
//!
//! The flood lasts seconds, not a minute, so CI stays fast; `scripts/bench-m6.sh`
//! and docs/benchmarks.md hold the long measurements.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use common::load::{client_loop, Sample};
use common::*;
use serde_json::json;

const DOWN: &str = "down.example.com";
const UP: &str = "up.example.com";
const FLOOD: Duration = Duration::from_secs(5);
const CLIENTS_PER_SITE: usize = 4;

struct Outcome {
    down: Vec<Sample>,
    up: Vec<Sample>,
}

/// Runs the flood against a proxy with the two sites set up; returns what clients saw.
fn flood(proxy: &Proxy, during: impl FnOnce()) -> Outcome {
    let stop = Arc::new(AtomicBool::new(false));
    let spawn_clients = |host: &'static str| -> Vec<thread::JoinHandle<Vec<Sample>>> {
        (0..CLIENTS_PER_SITE)
            .map(|_| {
                let (addr, stop) = (proxy.data, stop.clone());
                thread::spawn(move || client_loop(addr, host, "/x", stop))
            })
            .collect()
    };
    let down_clients = spawn_clients(DOWN);
    let up_clients = spawn_clients(UP);
    thread::sleep(FLOOD / 2);
    during();
    thread::sleep(FLOOD / 2);
    stop.store(true, Ordering::Relaxed);
    let collect = |clients: Vec<thread::JoinHandle<Vec<Sample>>>| {
        clients
            .into_iter()
            .flat_map(|client| client.join().unwrap())
            .collect::<Vec<_>>()
    };
    Outcome {
        down: collect(down_clients),
        up: collect(up_clients),
    }
}

fn set_up(dir: &std::path::Path, args: &[&str]) -> Proxy {
    let proxy = try_start_with_args(dir, free_port(), free_port(), ("", ""), args).unwrap();
    create_site(&proxy, "down", DOWN, free_port());
    create_site(
        &proxy,
        "up",
        UP,
        start_keepalive_backend("ok", Duration::ZERO),
    );
    proxy
}

fn sum_of(proxy: &Proxy, prefix: &str) -> f64 {
    let (_, text) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    text.lines()
        .filter(|line| line.starts_with(prefix))
        .filter_map(|line| line.rsplit(' ').next()?.parse::<f64>().ok())
        .sum()
}

fn no_baseline(proxy: &Proxy) {
    let (_, body) = admin(proxy, "GET", "/api/sampling", None);
    let mut settings = body["data"]["settings"].clone();
    for derived in ["builtInRedaction", "updatedBy", "updatedAtMs"] {
        settings.as_object_mut().unwrap().remove(derived);
    }
    settings["baselinePerMille"] = json!(0);
    assert_eq!(admin(proxy, "PUT", "/api/sampling", Some(settings)).0, 200);
}

/// How long a configuration write takes (a no-op settings write).
fn write_latency(proxy: &Proxy) -> Duration {
    let (_, body) = admin(proxy, "GET", "/api/sampling", None);
    let mut settings = body["data"]["settings"].clone();
    for derived in ["builtInRedaction", "updatedBy", "updatedAtMs"] {
        settings.as_object_mut().unwrap().remove(derived);
    }
    let started = Instant::now();
    assert_eq!(admin(proxy, "PUT", "/api/sampling", Some(settings)).0, 200);
    started.elapsed()
}

fn answered(samples: &[Sample], status: u16) -> usize {
    samples
        .iter()
        .filter(|sample| matches!(&sample.outcome, Ok((got, _)) if *got == status))
        .count()
}

#[test]
fn a_flood_of_connectivity_failures_costs_the_other_routes_nothing() {
    // The same load with sampling off, for a throughput comparison.
    let off_dir = tempfile::tempdir().unwrap();
    let off = set_up(off_dir.path(), &["--no-sampling"]);
    let without = flood(&off, || {});
    drop(off);

    let dir = tempfile::tempdir().unwrap();
    let proxy = set_up(dir.path(), &[]);
    no_baseline(&proxy);
    let idle_write = write_latency(&proxy);
    let mut flooded_write = Duration::ZERO;
    let with = flood(&proxy, || flooded_write = write_latency(&proxy));

    // No request failed or was answered wrongly because of sampling.
    assert_eq!(
        answered(&with.down, 502),
        with.down.len(),
        "every failure was a clean 502"
    );
    assert_eq!(
        answered(&with.up, 200),
        with.up.len(),
        "the healthy route never failed"
    );
    assert!(!with.up.is_empty() && !with.down.is_empty());

    // Throughput is within a generous factor of the same load without sampling.
    let total = |outcome: &Outcome| outcome.down.len() + outcome.up.len();
    assert!(
        total(&with) * 2 >= total(&without),
        "{} requests with sampling, {} without",
        total(&with),
        total(&without)
    );

    // Every candidate is either kept or counted as dropped, and the table is bounded
    // by the storm guard (the burst of 3, plus an interval's worth).
    let candidates = with.down.len() as f64;
    assert!(
        wait_until(|| {
            sum_of(&proxy, "fastproxy_samples_captured_total")
                + sum_of(&proxy, "fastproxy_samples_dropped_total")
                == candidates
        }),
        "captured + dropped = {} + {}, candidates = {candidates}",
        sum_of(&proxy, "fastproxy_samples_captured_total"),
        sum_of(&proxy, "fastproxy_samples_dropped_total"),
    );
    let rows = || {
        let (_, list) = admin(&proxy, "GET", "/api/samples?limit=100", None);
        list["data"].as_array().map_or(0, Vec::len)
    };
    assert!(wait_until(|| rows() >= 3), "{} rows", rows());
    assert!(rows() <= 8, "{} rows were stored", rows());

    // A configuration write during the flood was not starved.
    assert!(
        flooded_write < Duration::from_millis(500).max(idle_write * 30),
        "a write took {flooded_write:?} during the flood, {idle_write:?} idle"
    );
}

#[cfg(unix)]
#[test]
fn shutdown_flushes_what_the_flood_queued() {
    let dir = tempfile::tempdir().unwrap();
    // A batch interval longer than the test: only the shutdown flush can write.
    let mut proxy = start_on_free_ports(dir.path(), "", "\n[sampling]\nbatch_interval = \"60s\"\n");
    create_site(&proxy, "down", DOWN, free_port());
    for _ in 0..50 {
        assert_eq!(through_proxy(&proxy, DOWN, "/x").0, 502);
    }
    assert!(wait_until(|| metric(
        &proxy,
        "fastproxy_samples_captured_total{trigger=\"upstream_failure\"}"
    )
    .is_some_and(|kept| kept >= 3.0)));
    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, Duration::from_secs(20)),
        Some(true)
    );
    let db =
        fast_proxy_store::Database::open_read_only(&dir.path().join("state").join("fast-proxy.db"))
            .unwrap();
    let stored = db
        .read(|conn| {
            fast_proxy_store::repository::samples::list(
                conn,
                &fast_proxy_store::repository::samples::SampleFilter {
                    limit: Some(100),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    assert!(stored.len() >= 3, "{} samples were flushed", stored.len());
}
