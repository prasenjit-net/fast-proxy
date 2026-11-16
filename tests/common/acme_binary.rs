//! Running the real binary with ACME on against the in-process test CA: the
//! config lines, waiting for a status, and a TLS request that trusts only the
//! CA's issuer.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use fast_proxy_test_acme::TestAcme;
use fast_proxy_test_pki::{request, ClientHello};
use serde_json::{json, Value};

use super::{admin, Proxy};

/// `[data_plane]` lines turning TLS and ACME on against `ca`. `extra` is more
/// `[data_plane]` keys (it is placed before the subsections).
pub fn config_lines(ca: &TestAcme, https_port: u16, extra: &str) -> String {
    format!(
        "{extra}\n[data_plane.tls]\nenabled = true\nlisten = \"127.0.0.1:{https_port}\"\n\n\
         [data_plane.acme]\nenabled = true\ndirectory = {directory:?}\ndirectory_ca = {ca_file:?}\n\
         allowed_domains = [\"*.example.com\"]\nself_check = false\npropagation_wait = \"0s\"\n\
         renew_check_interval = \"1m\"\n",
        directory = ca.directory,
        ca_file = ca.directory_ca,
    )
}

/// Turns HTTPS and ACME on for the subsystem `id`.
pub fn turn_acme_on(proxy: &Proxy, id: i64, name: &str) {
    let (status, body) = admin(
        proxy,
        "PUT",
        &format!("/api/subsystems/{id}"),
        Some(json!({ "name": name, "https": { "enabled": true, "acmeEnabled": true } })),
    );
    assert_eq!(status, 200, "{body}");
}

/// Polls `GET /api/acme` until the subsystem's status satisfies `wanted`.
pub fn wait_for_status(
    proxy: &Proxy,
    name: &str,
    limit: Duration,
    wanted: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + limit;
    loop {
        let (_, body) = admin(proxy, "GET", "/api/acme", None);
        let found = body["data"]["subsystems"]
            .as_array()
            .and_then(|all| all.iter().find(|s| s["subsystem"] == name).cloned());
        if let Some(status) = found.filter(|status| wanted(status)) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "no wanted status for {name}: {body}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A `GET /` over TLS to `addr` for `host`, trusting only `issuer_pem`.
pub fn trusted_get(
    runtime: &tokio::runtime::Runtime,
    addr: SocketAddr,
    host: &str,
    issuer_pem: &str,
) -> String {
    let hello = ClientHello {
        sni: Some(host.into()),
        trust_pem: vec![issuer_pem.to_string()],
        ..ClientHello::default()
    };
    let raw = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    runtime
        .block_on(request(addr, &hello, raw.as_bytes()))
        .unwrap_or_else(|err| format!("handshake failed: {}", err.0))
}

/// Every file under `dir` with its bytes.
pub fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                found.push((path.display().to_string(), bytes));
            }
        }
    }
    found
}
