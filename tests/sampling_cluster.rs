//! Request sampling with two real processes sharing one database: settings
//! follow the poll, either instance lists the other's samples (naming the
//! instance), a sample write is not a configuration change (no revision, no
//! reload), clearing on one empties the table for both, and two processes
//! inserting and pruning one file together lose nothing and fail nothing.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::console::{printed, start_ok_with_config, token_in, Running};
use common::{free_port, http, http_with_headers, start_backend};
use serde_json::{json, Value};

const KEY_ONE: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const KEY_TWO: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
const HOST: &str = "shop.example.com";

fn instance(own: &Path, shared: &Path, master_key: &str) -> Running {
    let extra = format!(
        "database = {:?}\nconfig_poll_interval = \"200ms\"\n",
        shared.join("shared.db")
    ) + "\n[sampling]\nprune_interval = \"200ms\"\n";
    start_ok_with_config(own, &extra, &[("FAST_PROXY_MASTER_KEY", master_key)], &[])
}

fn raw(
    running: &Running,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String) {
    let authorization = format!("Bearer {token}");
    http_with_headers(
        running.admin,
        method,
        path,
        "localhost",
        &[("Authorization", &authorization)],
        body.as_ref(),
    )
}

fn call(
    running: &Running,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let (status, text) = raw(running, token, method, path, body);
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("never happened: {what}");
}

struct Cluster {
    a: Running,
    b: Running,
    token: String,
    _dirs: Vec<tempfile::TempDir>,
}

/// Two instances on one database, with a site on `HOST` whose backend is dead
/// (or alive, with `live_backend`), made through A.
fn cluster(live_backend: bool) -> Cluster {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let a = instance(dirs[0].path(), dirs[2].path(), KEY_ONE);
    let token = token_in(&printed(&a)).expect("the first instance prints the bootstrap token");
    let b = instance(dirs[1].path(), dirs[2].path(), KEY_TWO);
    let backend = if live_backend {
        start_backend("ok")
    } else {
        free_port()
    };
    let (_, created) = call(
        &a,
        &token,
        "POST",
        "/api/subsystems",
        Some(json!({ "name": "shop" })),
    );
    let base = format!("/api/subsystems/{}", created["data"]["id"]);
    call(
        &a,
        &token,
        "POST",
        &format!("{base}/hosts"),
        Some(json!({ "pattern": HOST })),
    );
    let (_, group) = call(
        &a,
        &token,
        "POST",
        &format!("{base}/upstreams"),
        Some(
            json!({ "name": "backend", "endpoints": [{ "addr": format!("127.0.0.1:{backend}") }] }),
        ),
    );
    call(
        &a,
        &token,
        "POST",
        &format!("{base}/routes"),
        Some(json!({ "prefix": "/", "upstreamGroupId": group["data"]["id"] })),
    );
    let cluster = Cluster {
        a,
        b,
        token,
        _dirs: dirs,
    };
    let expected = if live_backend { 200 } else { 502 };
    eventually("B serves the site made on A", || {
        http(cluster.b.data, "GET", "/", HOST, None).0 == expected
    });
    cluster
}

impl Cluster {
    fn samples(&self, on: &Running) -> Vec<Value> {
        let (_, body) = call(on, &self.token, "GET", "/api/samples?limit=100", None);
        body["data"].as_array().cloned().unwrap_or_default()
    }

    fn settings(&self, on: &Running) -> Value {
        call(on, &self.token, "GET", "/api/sampling", None).1["data"]["settings"].clone()
    }

    fn write_settings(&self, on: &Running, change: impl FnOnce(&mut Value)) {
        let mut settings = self.settings(on);
        for derived in ["builtInRedaction", "updatedBy", "updatedAtMs"] {
            settings.as_object_mut().unwrap().remove(derived);
        }
        change(&mut settings);
        let (status, body) = call(on, &self.token, "PUT", "/api/sampling", Some(settings));
        assert_eq!(status, 200, "{body}");
    }

    fn metric(&self, on: &Running, name: &str) -> f64 {
        let (_, text) = raw(on, &self.token, "GET", "/metrics", None);
        text.lines()
            .filter(|line| line.starts_with(name))
            .filter_map(|line| line.rsplit(' ').next()?.parse::<f64>().ok())
            .sum()
    }

    fn config_version(&self, on: &Running) -> i64 {
        call(on, &self.token, "GET", "/api/sampling", None).1["version"]
            .as_i64()
            .unwrap()
    }
}

#[test]
fn a_settings_change_on_one_instance_reaches_the_other_within_the_poll() {
    let cluster = cluster(false);
    cluster.write_settings(&cluster.a, |settings| {
        settings["slowThresholdMs"] = json!(4321)
    });
    eventually("B shows the new threshold", || {
        cluster.settings(&cluster.b)["slowThresholdMs"] == json!(4321)
    });
}

#[test]
fn either_instance_lists_the_samples_of_both_and_names_the_writer() {
    let cluster = cluster(false);
    assert_eq!(http(cluster.a.data, "GET", "/from-a", HOST, None).0, 502);
    assert_eq!(http(cluster.b.data, "GET", "/from-b", HOST, None).0, 502);
    for viewer in [&cluster.a, &cluster.b] {
        eventually("both samples are listed", || {
            cluster.samples(viewer).len() >= 2
        });
        let kept = cluster.samples(viewer);
        let instances: std::collections::BTreeSet<_> = kept
            .iter()
            .map(|s| s["instance"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            instances.len(),
            2,
            "each sample names the instance that wrote it: {kept:?}"
        );
    }
}

#[test]
fn a_sample_write_is_not_a_configuration_change() {
    let cluster = cluster(false);
    let (version, reloads) = (
        cluster.config_version(&cluster.b),
        cluster.metric(&cluster.b, "fastproxy_config_reloads_total"),
    );
    for _ in 0..3 {
        assert_eq!(http(cluster.a.data, "GET", "/x", HOST, None).0, 502);
    }
    eventually("A stored its sample", || {
        !cluster.samples(&cluster.b).is_empty()
    });
    std::thread::sleep(Duration::from_millis(800)); // four polls of B
    assert_eq!(
        cluster.config_version(&cluster.b),
        version,
        "no revision was made"
    );
    assert_eq!(
        cluster.metric(&cluster.b, "fastproxy_config_reloads_total"),
        reloads,
        "B did not reload"
    );
}

#[test]
fn clearing_on_one_instance_empties_the_table_for_both() {
    let cluster = cluster(false);
    assert_eq!(http(cluster.a.data, "GET", "/x", HOST, None).0, 502);
    eventually("B lists A's sample", || {
        !cluster.samples(&cluster.b).is_empty()
    });
    let (status, body) = call(&cluster.b, &cluster.token, "DELETE", "/api/samples", None);
    assert_eq!(status, 200, "{body}");
    assert!(cluster.samples(&cluster.a).is_empty());
    assert!(cluster.samples(&cluster.b).is_empty());
}

#[test]
fn two_processes_inserting_and_pruning_one_file_lose_and_fail_nothing() {
    let cluster = cluster(true);
    cluster.write_settings(&cluster.a, |settings| {
        settings["triggers"] = json!(["baseline"]);
        settings["baselinePerMille"] = json!(1000);
        settings["ratePerSec"] = json!(10000);
        settings["rateBurst"] = json!(10000);
        settings["maxRows"] = json!(100);
    });
    // B's data plane applies the change at its next poll: probe until it samples.
    eventually("B's data plane samples with the new settings", || {
        assert_eq!(http(cluster.b.data, "GET", "/probe", HOST, None).0, 200);
        cluster.metric(&cluster.b, "fastproxy_samples_captured_total") > 0.0
    });
    let threads: Vec<_> = [cluster.a.data, cluster.b.data]
        .into_iter()
        .map(|addr| {
            std::thread::spawn(move || {
                for _ in 0..300 {
                    assert_eq!(http(addr, "GET", "/x", HOST, None).0, 200);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let rows = || {
        call(&cluster.b, &cluster.token, "GET", "/api/sampling", None).1["data"]["status"]["table"]
            ["rows"]
            .as_i64()
            .unwrap()
    };
    let written = || {
        cluster.metric(&cluster.a, "fastproxy_samples_written_total")
            + cluster.metric(&cluster.b, "fastproxy_samples_written_total")
    };
    eventually("every kept sample was written", || written() >= 600.0);
    eventually("the pruners bring the table to its row limit", || {
        rows() <= 100
    });
    for instance in [&cluster.a, &cluster.b] {
        assert_eq!(
            cluster.metric(
                instance,
                "fastproxy_samples_dropped_total{reason=\"store_error\"}"
            ),
            0.0,
            "no write failed"
        );
    }
}
