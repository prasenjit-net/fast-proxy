//! Admin-API helpers for upstream groups and their endpoints.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::{admin, Proxy};

/// A subsystem answering to `host` whose `/` route goes to a new group of
/// `ports` (one endpoint each). `policy` holds extra group fields such as
/// `lb` or `health`. Returns the subsystem and group ids.
pub fn create_group_site(
    proxy: &Proxy,
    name: &str,
    host: &str,
    ports: &[u16],
    policy: Value,
) -> (i64, i64) {
    let (status, body) = admin(
        proxy,
        "POST",
        "/api/subsystems",
        Some(json!({ "name": name })),
    );
    assert_eq!(status, 201, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let base = format!("/api/subsystems/{id}");
    let (status, _) = admin(
        proxy,
        "POST",
        &format!("{base}/hosts"),
        Some(json!({ "pattern": host })),
    );
    assert_eq!(status, 201);
    let mut group = policy;
    group["name"] = json!("backend");
    group["endpoints"] = Value::Array(
        ports
            .iter()
            .map(|port| json!({ "addr": format!("127.0.0.1:{port}") }))
            .collect(),
    );
    let (status, body) = admin(proxy, "POST", &format!("{base}/upstreams"), Some(group));
    assert_eq!(status, 201, "{body}");
    let upstream = body["data"]["id"].as_i64().unwrap();
    let (status, body) = admin(
        proxy,
        "POST",
        &format!("{base}/routes"),
        Some(json!({ "prefix": "/", "upstreamGroupId": upstream })),
    );
    assert_eq!(status, 201, "{body}");
    (id, upstream)
}

/// Endpoint ids and addresses of a group, in creation order.
pub fn endpoints_of(proxy: &Proxy, subsystem: i64, upstream: i64) -> Vec<(i64, String)> {
    let (_, body) = admin(
        proxy,
        "GET",
        &format!("/api/subsystems/{subsystem}/upstreams"),
        None,
    );
    let group = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["id"] == upstream)
        .unwrap();
    group["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["id"].as_i64().unwrap(),
                e["addr"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// The endpoint id for the backend on `port`.
pub fn endpoint_on(proxy: &Proxy, subsystem: i64, upstream: i64, port: u16) -> i64 {
    let addr = format!("127.0.0.1:{port}");
    endpoints_of(proxy, subsystem, upstream)
        .into_iter()
        .find(|(_, a)| *a == addr)
        .unwrap_or_else(|| panic!("no endpoint on {addr}"))
        .0
}

pub fn set_endpoint_state(
    proxy: &Proxy,
    subsystem: i64,
    upstream: i64,
    endpoint: i64,
    state: &str,
) {
    let (status, body) = admin(
        proxy,
        "PUT",
        &format!("/api/subsystems/{subsystem}/upstreams/{upstream}/endpoints/{endpoint}/state"),
        Some(json!({ "state": state })),
    );
    assert_eq!(status, 200, "{body}");
}

/// The group's live status as the admin API reports it.
pub fn upstream_status(proxy: &Proxy, subsystem: i64, upstream: i64) -> Value {
    let (status, body) = admin(
        proxy,
        "GET",
        &format!("/api/subsystems/{subsystem}/upstreams/{upstream}/status"),
        None,
    );
    assert_eq!(status, 200, "{body}");
    body["data"].clone()
}

/// What the proxy does with the endpoint on `port` right now (`healthy`,
/// `unhealthy`, `ejected`, `draining` or `disabled`).
pub fn effective_state(proxy: &Proxy, subsystem: i64, upstream: i64, port: u16) -> String {
    let addr = format!("127.0.0.1:{port}");
    let status = upstream_status(proxy, subsystem, upstream);
    status["endpoints"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["addr"] == addr.as_str())
        .map(|e| e["effective"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

/// Polls `condition` every 25 ms for up to `seconds`.
pub fn wait_within(seconds: u64, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

/// A `GET` with extra request headers (each `Name: value`), over a fresh connection.
pub fn get_with_headers(proxy: &Proxy, host: &str, path: &str, headers: &[&str]) -> (u16, String) {
    let extra: String = headers.iter().map(|h| format!("{h}\r\n")).collect();
    let mut stream = TcpStream::connect(proxy.data).unwrap();
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(20))).ok();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n")
                .as_bytes(),
        )
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    (
        head.split_whitespace().nth(1).unwrap().parse().unwrap(),
        body.to_string(),
    )
}

/// Gives the endpoint on `port` a new weight, keeping its address.
pub fn set_weight(proxy: &Proxy, subsystem: i64, upstream: i64, port: u16, weight: u32) {
    let endpoint = endpoint_on(proxy, subsystem, upstream, port);
    let (status, body) = admin(
        proxy,
        "PUT",
        &format!("/api/subsystems/{subsystem}/upstreams/{upstream}/endpoints/{endpoint}"),
        Some(json!({ "addr": format!("127.0.0.1:{port}"), "weight": weight })),
    );
    assert_eq!(status, 200, "{body}");
}
