//! Two real `fast-proxy` processes sharing one database, each with its own
//! master key (tokens carry no encrypted data, so that must not matter): a token
//! made on one works on the other and a revocation locks it out there too,
//! maintenance and its expiry follow, and the audit log is shared with every
//! entry naming the instance that wrote it.

mod common;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use common::console::{bootstrap_token, printed, start_ok_with_config, token_in, Running};
use common::{http, http_with_headers, start_backend};
use serde_json::{json, Value};

const KEY_ONE: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const KEY_TWO: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";
const HOST: &str = "shop.example.com";

/// Starts an instance with its own state directory and master key, sharing the
/// database file under `shared`, polling it every 200 ms.
fn instance(own: &Path, shared: &Path, master_key: &str) -> Running {
    let extra = format!(
        "database = {:?}\nconfig_poll_interval = \"200ms\"\n",
        shared.join("shared.db")
    );
    start_ok_with_config(own, &extra, &[("FAST_PROXY_MASTER_KEY", master_key)], &[])
}

fn call(
    running: &Running,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let authorization = format!("Bearer {token}");
    let (status, text) = http_with_headers(
        running.admin,
        method,
        path,
        "localhost",
        &[("Authorization", &authorization)],
        body.as_ref(),
    );
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

/// Retries `check` until it holds, for at most 30 s (generous for slow CI).
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

fn data_status(running: &Running) -> u16 {
    http(running.data, "GET", "/", HOST, None).0
}

#[test]
fn tokens_maintenance_and_the_audit_log_are_shared_by_two_processes() {
    let shared = tempfile::tempdir().unwrap();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = instance(dir_a.path(), shared.path(), KEY_ONE);
    let token = bootstrap_token(&a).expect("the first instance prints the bootstrap token");
    let b = instance(dir_b.path(), shared.path(), KEY_TWO);
    assert!(
        token_in(&printed(&b)).is_none(),
        "the second instance finds an admin token and prints none"
    );
    assert_eq!(call(&b, &token, "GET", "/api/status", None).0, 200);

    // A site made on A reaches B.
    let backend = start_backend("shop-backend");
    let (status, created) = call(
        &a,
        &token,
        "POST",
        "/api/subsystems",
        Some(json!({ "name": "shop" })),
    );
    assert_eq!(status, 201, "{created}");
    let id = created["data"]["id"].as_i64().unwrap();
    let base = format!("/api/subsystems/{id}");
    assert_eq!(
        call(
            &a,
            &token,
            "POST",
            &format!("{base}/hosts"),
            Some(json!({ "pattern": HOST }))
        )
        .0,
        201
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
    let upstream = group["data"]["id"].as_i64().unwrap();
    assert_eq!(
        call(
            &a,
            &token,
            "POST",
            &format!("{base}/routes"),
            Some(json!({ "prefix": "/", "upstreamGroupId": upstream }))
        )
        .0,
        201
    );
    eventually("B serves the site made on A", || data_status(&b) == 200);

    // A token made on A works on B; revoking it on A locks it out on B.
    let (status, made) = call(
        &a,
        &token,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "ci", "role": "admin" })),
    );
    assert_eq!(status, 201, "{made}");
    let (ci_id, ci) = (
        made["data"]["id"].as_i64().unwrap(),
        made["data"]["secret"].as_str().unwrap().to_string(),
    );
    eventually("B accepts the token made on A", || {
        call(&b, &ci, "GET", "/api/status", None).0 == 200
    });
    assert_eq!(
        call(&a, &token, "DELETE", &format!("/api/tokens/{ci_id}"), None).0,
        200
    );
    eventually("B refuses the token revoked on A", || {
        call(&b, &ci, "GET", "/api/status", None).0 == 401
    });

    // Maintenance turned on at A closes B too, and its expiry reopens both.
    let (status, reply) = call(
        &a,
        &token,
        "PUT",
        "/api/maintenance/global",
        Some(json!({ "message": "cluster", "ttlSecs": 2 })),
    );
    assert_eq!(status, 200, "{reply}");
    eventually("B answers with the maintenance page", || {
        data_status(&b) == 503
    });
    assert_eq!(data_status(&a), 503);
    eventually("the TTL reopens A", || data_status(&a) == 200);
    eventually("the TTL reopens B", || data_status(&b) == 200);
    eventually("the expired scope is gone for B's API", || {
        call(&b, &token, "GET", "/api/maintenance", None).1["data"] == json!([])
    });

    // The audit log is one log; each entry names the process that wrote it.
    let (status, from_b) = call(
        &b,
        &token,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "from-b", "role": "viewer" })),
    );
    assert_eq!(status, 201, "{from_b}");
    let (_, audit) = call(&a, &token, "GET", "/api/audit?limit=200", None);
    let entries = audit["data"].as_array().unwrap();
    let instance_of = |action: &str, target: &str| -> String {
        entries
            .iter()
            .find(|entry| entry["action"] == action && entry["target"] == target)
            .unwrap_or_else(|| panic!("no {action} {target} in {entries:#?}"))["instance"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let on_a = instance_of("token.create", "ci");
    let on_b = instance_of("token.create", "from-b");
    assert!(!on_a.is_empty() && !on_b.is_empty());
    assert_ne!(on_a, on_b, "two processes, two instance names");
    let seen: BTreeSet<&str> = entries
        .iter()
        .filter_map(|entry| entry["instance"].as_str())
        .collect();
    assert!(seen.len() >= 2, "{seen:?}");
    assert_eq!(instance_of("token.revoke", &format!("token:{ci_id}")), on_a);
}
