//! M5's exit criteria against the real binary with authentication required:
//! each maintenance scope closes only what it covers (503, `Retry-After`, no
//! request reaches the upstream), a bypass lets the right requests through and
//! cannot be forged, health checks keep running, the audit trail names who did
//! what (refusals too), a token can be created, used and revoked, and a flood of
//! bad tokens is slowed down.
//!
//! Waits are on events, never on a fixed sleep.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use common::console::{printed, start_ok, token_in, Running};
use common::flaky::FlakyBackend;
use common::{http_with_headers, wait_until};
use serde_json::{json, Value};

/// A started proxy and the bootstrap token it printed.
struct Ops {
    running: Running,
    bootstrap: String,
    _dir: tempfile::TempDir,
}

fn start() -> Ops {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok(dir.path(), &[]);
    let bootstrap = token_in(&printed(&running)).expect("the bootstrap token is printed");
    Ops {
        running,
        bootstrap,
        _dir: dir,
    }
}

impl Ops {
    fn call_as(&self, token: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let authorization = format!("Bearer {token}");
        let (status, text) = http_with_headers(
            self.running.admin,
            method,
            path,
            "localhost",
            &[("Authorization", &authorization)],
            body.as_ref(),
        );
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.call_as(&self.bootstrap, method, path, body)
    }

    /// A token with `role`, created over the API; returns its id and secret.
    fn make_token(&self, name: &str, role: &str) -> (i64, String) {
        let (status, body) = self.call(
            "POST",
            "/api/tokens",
            Some(json!({ "name": name, "role": role })),
        );
        assert_eq!(status, 201, "{body}");
        (
            body["data"]["id"].as_i64().unwrap(),
            body["data"]["secret"].as_str().unwrap().to_string(),
        )
    }

    /// A subsystem answering to `host` with a `/` route to `backend`; returns
    /// the subsystem, upstream and route ids.
    fn site(&self, name: &str, host: &str, backend: u16, policy: Value) -> (i64, i64, i64) {
        let (status, body) = self.call("POST", "/api/subsystems", Some(json!({ "name": name })));
        assert_eq!(status, 201, "{body}");
        let id = body["data"]["id"].as_i64().unwrap();
        let base = format!("/api/subsystems/{id}");
        let (status, _) = self.call(
            "POST",
            &format!("{base}/hosts"),
            Some(json!({ "pattern": host })),
        );
        assert_eq!(status, 201);
        let mut group = policy;
        group["name"] = json!("backend");
        group["endpoints"] = json!([{ "addr": format!("127.0.0.1:{backend}") }]);
        let (status, body) = self.call("POST", &format!("{base}/upstreams"), Some(group));
        assert_eq!(status, 201, "{body}");
        let upstream = body["data"]["id"].as_i64().unwrap();
        let route = self.route(id, "/", upstream);
        (id, upstream, route)
    }

    fn route(&self, subsystem: i64, prefix: &str, upstream: i64) -> i64 {
        let (status, body) = self.call(
            "POST",
            &format!("/api/subsystems/{subsystem}/routes"),
            Some(json!({ "prefix": prefix, "upstreamGroupId": upstream })),
        );
        assert_eq!(status, 201, "{body}");
        body["data"]["id"].as_i64().unwrap()
    }

    fn get(&self, host: &str, path: &str) -> Reply {
        self.get_with(host, path, &[])
    }

    fn get_with(&self, host: &str, path: &str, headers: &[(&str, &str)]) -> Reply {
        data_request(self.running.data, host, path, headers)
    }

    fn audit(&self) -> Vec<Value> {
        let (status, body) = self.call("GET", "/api/audit?limit=200", None);
        assert_eq!(status, 200, "{body}");
        body["data"].as_array().unwrap().clone()
    }
}

/// What the data plane answered: status, headers (lowercase names) and body.
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// One GET to the data plane in one write, returning the headers too.
fn data_request(addr: SocketAddr, host: &str, path: &str, headers: &[(&str, &str)]) -> Reply {
    let extra: String = headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_nodelay(true).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_string(),
    }
}

const SHOP: &str = "shop.example.com";
const BLOG: &str = "blog.example.com";

// ---- each scope closes only what it covers ----

#[test]
fn each_scope_closes_only_what_it_covers_and_nothing_reaches_the_upstream() {
    let ops = start();
    let shop_backend = FlakyBackend::start("shop-backend");
    let blog_backend = FlakyBackend::start("blog-backend");
    let (shop, shop_upstream, _) = ops.site("shop", SHOP, shop_backend.port, json!({}));
    let (blog, _, _) = ops.site("blog", BLOG, blog_backend.port, json!({}));
    let api_route = ops.route(shop, "/api", shop_upstream);
    assert_eq!(ops.get(SHOP, "/api/x").status, 200);
    assert_eq!(ops.get(SHOP, "/").status, 200);
    assert_eq!(ops.get(BLOG, "/").status, 200);

    // One route: only its prefix is closed.
    let route_path = format!("/api/subsystems/{shop}/routes/{api_route}/maintenance");
    let (status, body) = ops.call(
        "PUT",
        &route_path,
        Some(json!({ "message": "api only", "ttlSecs": 600 })),
    );
    assert_eq!(status, 200, "{body}");
    let closed = ops.get(SHOP, "/api/x");
    assert_eq!(closed.status, 503);
    assert!(closed.body.contains("api only"), "{}", closed.body);
    let retry: u64 = closed.header("retry-after").unwrap().parse().unwrap();
    assert!((1..=600).contains(&retry), "retry-after {retry}");
    assert_eq!(closed.header("cache-control"), Some("no-store"));
    assert_eq!(ops.get(SHOP, "/").status, 200, "the rest of the subsystem");
    assert_eq!(ops.get(BLOG, "/").status, 200, "another subsystem");
    assert_eq!(ops.call("DELETE", &route_path, None).0, 200);
    assert_eq!(ops.get(SHOP, "/api/x").status, 200);

    // One subsystem: all its hosts, nobody else.
    let shop_path = format!("/api/subsystems/{blog}/maintenance");
    assert_eq!(ops.call("PUT", &shop_path, Some(json!({}))).0, 200);
    assert_eq!(ops.get(BLOG, "/").status, 503);
    assert_eq!(ops.get(SHOP, "/").status, 200);
    assert_eq!(ops.call("DELETE", &shop_path, None).0, 200);
    assert_eq!(ops.get(BLOG, "/").status, 200);

    // The whole proxy, with a JSON page of the operator's own.
    let (status, body) = ops.call(
        "PUT",
        "/api/maintenance/global",
        Some(json!({
            "body": r#"{"status":"maintenance"}"#,
            "contentType": "application/json",
            "retryAfterSecs": 45
        })),
    );
    assert_eq!(status, 200, "{body}");
    let (shop_before, blog_before) = (shop_backend.hits(), blog_backend.hits());
    for host in [SHOP, BLOG] {
        for path in ["/", "/api/x", "/anything"] {
            let reply = ops.get(host, path);
            assert_eq!(reply.status, 503, "{host}{path}");
            assert_eq!(reply.body, r#"{"status":"maintenance"}"#);
            assert!(reply
                .header("content-type")
                .is_some_and(|kind| kind.starts_with("application/json")));
            assert_eq!(reply.header("retry-after"), Some("45"));
        }
    }
    assert_eq!(
        (shop_backend.hits(), blog_backend.hits()),
        (shop_before, blog_before),
        "a request in maintenance never reaches an upstream"
    );
    assert_eq!(ops.call("DELETE", "/api/maintenance/global", None).0, 200);
    assert_eq!(ops.get(SHOP, "/").status, 200);
    assert_eq!(ops.get(BLOG, "/").status, 200);
}

// ---- bypass ----

#[test]
fn a_bypass_by_header_or_address_lets_the_right_requests_through_and_cannot_be_forged() {
    let ops = start();
    let backend = FlakyBackend::start("shop-backend");
    let (shop, _, _) = ops.site("shop", SHOP, backend.port, json!({}));
    let path = format!("/api/subsystems/{shop}/maintenance");

    let (status, body) = ops.call(
        "PUT",
        &path,
        Some(json!({ "bypass": { "cidrs": ["10.0.0.0/8"], "header": "X-Bypass" } })),
    );
    assert_eq!(status, 200, "{body}");
    let secret = body["bypassSecret"].as_str().expect("a generated secret");

    assert_eq!(ops.get(SHOP, "/").status, 503);
    // The caller here is 127.0.0.1; a claimed address in a header is not trusted.
    let forged = ops.get_with(SHOP, "/", &[("X-Forwarded-For", "10.1.2.3")]);
    assert_eq!(forged.status, 503, "a forged X-Forwarded-For");
    assert_eq!(
        ops.get_with(SHOP, "/", &[("X-Bypass", "not-the-secret")])
            .status,
        503
    );
    assert_eq!(
        ops.get_with(SHOP, "/", &[("X-Other", secret)]).status,
        503,
        "the right secret in the wrong header"
    );
    let before = backend.hits();
    assert_eq!(ops.get_with(SHOP, "/", &[("X-Bypass", secret)]).status, 200);
    assert_eq!(backend.hits(), before + 1, "a bypassed request is served");

    // Replacing the scope forgets the old secret and honours the new network.
    let (status, _) = ops.call(
        "PUT",
        &path,
        Some(json!({ "bypass": { "cidrs": ["127.0.0.0/8"] } })),
    );
    assert_eq!(status, 200);
    assert_eq!(ops.get(SHOP, "/").status, 200, "the caller's own network");
    assert_eq!(
        ops.get_with(SHOP, "/", &[("X-Bypass", secret)]).status,
        200,
        "served by the network rule, whatever the header says"
    );
}

// ---- health checks ----

#[test]
fn health_checks_keep_running_while_the_whole_proxy_is_in_maintenance() {
    let ops = start();
    let backend = FlakyBackend::start("shop-backend");
    ops.site(
        "shop",
        SHOP,
        backend.port,
        json!({ "health": {
            "check": "http", "path": "/healthz",
            "intervalMs": 200, "timeoutMs": 200, "rise": 2, "fall": 2
        } }),
    );
    assert_eq!(
        ops.call("PUT", "/api/maintenance/global", Some(json!({})))
            .0,
        200
    );
    let (probes, hits) = (backend.probes(), backend.hits());
    assert!(
        wait_until(|| backend.probes() >= probes + 3),
        "probes stopped during maintenance"
    );
    assert_eq!(ops.get(SHOP, "/").status, 503);
    assert_eq!(backend.hits(), hits, "probes are not requests");

    // The group is ready the moment maintenance ends.
    assert_eq!(ops.call("DELETE", "/api/maintenance/global", None).0, 200);
    assert_eq!(ops.get(SHOP, "/").status, 200);
}

// ---- the audit trail ----

#[test]
fn the_audit_trail_names_who_did_what_and_records_the_refusals() {
    let ops = start();
    let backend = FlakyBackend::start("shop-backend");
    ops.site("shop", SHOP, backend.port, json!({}));
    let (_, operator) = ops.make_token("operator", "admin");
    let (_, watcher) = ops.make_token("watcher", "viewer");

    assert_eq!(
        ops.call_as(
            &operator,
            "PUT",
            "/api/maintenance/global",
            Some(json!({ "bypass": { "header": "X-Bypass" } }))
        )
        .0,
        200
    );
    assert_eq!(
        ops.call_as(&operator, "DELETE", "/api/maintenance/global", None)
            .0,
        200
    );
    assert_eq!(
        ops.call_as(&watcher, "PUT", "/api/maintenance/global", Some(json!({})))
            .0,
        403
    );
    assert_eq!(ops.call_as("fp_1.wrong", "GET", "/api/status", None).0, 401);

    let entries = ops.audit();
    let find = |action: &str| -> &Value {
        entries
            .iter()
            .find(|entry| entry["action"] == action)
            .unwrap_or_else(|| panic!("no {action} entry in {entries:#?}"))
    };
    for action in ["maintenance.set", "maintenance.clear"] {
        let entry = find(action);
        assert_eq!(entry["actor"], "operator", "{action}");
        assert_eq!(entry["role"], "admin");
        assert_eq!(entry["outcome"], "ok");
        assert_eq!(entry["source"], "127.0.0.1");
        assert!(entry["revision"].is_i64() || entry["revision"].is_u64());
    }
    let denied = find("access.denied");
    assert_eq!(
        (denied["actor"].as_str(), denied["role"].as_str()),
        (Some("watcher"), Some("viewer"))
    );
    assert_eq!(denied["outcome"], "denied");
    let unauthenticated = find("auth.failed");
    assert_eq!(unauthenticated["outcome"], "denied");
    assert_eq!(unauthenticated["actor"], "anonymous");
    assert_eq!(unauthenticated["detail"]["status"], 401);
    let created: Vec<&str> = entries
        .iter()
        .filter(|entry| entry["action"] == "token.create")
        .map(|entry| entry["target"].as_str().unwrap())
        .collect();
    assert!(
        created.contains(&"operator") && created.contains(&"watcher"),
        "{created:?}"
    );

    // The metrics count what the guard did and how many entries were written.
    let (status, _) = http_with_headers(
        ops.running.admin,
        "GET",
        "/api/status",
        "evil.example",
        &[],
        None,
    );
    assert_eq!(status, 421);
    let authorization = format!("Bearer {}", ops.bootstrap);
    let (status, metrics) = http_with_headers(
        ops.running.admin,
        "GET",
        "/metrics",
        "localhost",
        &[("Authorization", &authorization)],
        None,
    );
    assert_eq!(status, 200);
    let series = |name: &str, label: &str, value: &str| -> f64 {
        let prefix = format!("{name}{{{label}=\"{value}\"}} ");
        metrics
            .lines()
            .find_map(|line| line.strip_prefix(&prefix)?.parse().ok())
            .unwrap_or(0.0)
    };
    let requests = |result: &str| series("fastproxy_admin_requests_total", "result", result);
    let written = |outcome: &str| series("fastproxy_audit_entries_total", "outcome", outcome);
    assert_eq!(requests("unauthenticated"), 1.0, "the one wrong token");
    assert_eq!(requests("forbidden"), 1.0, "the viewer's write");
    assert_eq!(requests("host_refused"), 1.0, "the foreign Host");
    assert!(requests("ok") >= 6.0, "served requests: {}", requests("ok"));
    // Two token creations, a site, a maintenance on and off by the operator; then the
    // refusals: the viewer's denial, the bad token and the foreign host.
    assert!(written("ok") >= 6.0, "committed entries: {}", written("ok"));
    assert!(written("denied") >= 3.0, "refusals: {}", written("denied"));
    assert_eq!(
        written("ok") + written("denied") + written("failed"),
        entries.len() as f64 + 1.0,
        "every entry is counted once (the log read above was taken before the foreign Host)"
    );

    // The denied write changed nothing, and no secret is anywhere in the trail.
    assert_eq!(ops.get(SHOP, "/").status, 200);
    let text = serde_json::to_string(&entries).unwrap();
    for secret in [&operator, &watcher, &ops.bootstrap] {
        let part = secret
            .split_once('.')
            .map_or(secret.as_str(), |(_, rest)| rest);
        assert!(!text.contains(part), "a token secret is in the audit log");
    }
}

// ---- tokens ----

#[test]
fn a_token_is_created_used_and_revoked_and_the_last_admin_is_protected() {
    let ops = start();
    let (id, secret) = ops.make_token("ci", "admin");
    assert_eq!(ops.call_as(&secret, "GET", "/api/status", None).0, 200);

    let (status, listed) = ops.call("GET", "/api/tokens", None);
    assert_eq!(status, 200);
    assert!(
        listed.to_string().contains("\"ci\"") && !listed.to_string().contains(&secret),
        "listed by name, never by secret"
    );
    let (status, _) = ops.call("DELETE", &format!("/api/tokens/{id}"), None);
    assert_eq!(status, 200);
    assert_eq!(
        ops.call_as(&secret, "GET", "/api/status", None).0,
        401,
        "a revoked token is refused at once"
    );

    // The bootstrap token is now the only admin: revoking it is refused.
    let (_, listed) = ops.call("GET", "/api/tokens", None);
    let bootstrap_id = listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|token| token["revokedAtMs"].is_null() && token["role"] == "admin")
        .map(|token| token["id"].as_i64().unwrap())
        .unwrap();
    let (status, body) = ops.call("DELETE", &format!("/api/tokens/{bootstrap_id}"), None);
    assert_eq!(status, 409, "{body}");
    assert_eq!(ops.call("GET", "/api/status", None).0, 200);
}

// ---- the throttle ----

#[test]
fn a_flood_of_wrong_tokens_is_slowed_down_even_for_the_right_one() {
    let ops = start();
    let statuses: Vec<u16> = (0..12)
        .map(|_| ops.call_as("fp_1.wrong", "GET", "/api/status", None).0)
        .collect();
    assert!(
        statuses[..10].iter().all(|status| *status == 401),
        "{statuses:?}"
    );
    assert_eq!(statuses[10], 429, "{statuses:?}");
    assert_eq!(
        ops.call("GET", "/api/status", None).0,
        429,
        "a flood cannot be used to guess faster"
    );
}
