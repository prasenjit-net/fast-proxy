//! Secret hygiene against the real binary: after tokens are created, bypass
//! secrets set and a certificate with its private key uploaded, no secret is in
//! any file the proxy wrote (the database and its write-ahead log included), in
//! any API response, metric, WebSocket event, audit entry, revision, log line or
//! the access log. The one place a bootstrap token may appear is the console,
//! once, where it is printed for the operator.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use common::console::{printed, start_ok_with_args, token_in, Running};
use common::{http_with_headers, wait_until};
use fast_proxy_test_pki::{LeafSpec, TestCa};
use serde_json::{json, Value};

fn call(
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

fn json_of(reply: (u16, String)) -> Value {
    assert!(reply.0 < 300, "{reply:?}");
    serde_json::from_str(&reply.1).unwrap()
}

/// The part of a token after `fp_<id>.`: the secret itself.
fn secret_of(token: &str) -> String {
    token.split_once('.').unwrap().1.to_string()
}

/// Every file under `dir`, with its bytes; unreadable ones are an error.
fn files_under(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path)
                    .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
                found.push((path.display().to_string(), bytes));
            }
        }
    }
    found
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// A WebSocket client that records every byte the server sends.
struct Listener {
    received: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Listener {
    fn open(running: &Running, ticket: &str) -> Self {
        let received = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let mut stream = TcpStream::connect(running.admin).unwrap();
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        stream
            .write_all(
                format!(
                    "GET /ws?ticket={ticket} HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\n\
                     Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
                     Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        let (sink, flag) = (Arc::clone(&received), Arc::clone(&stop));
        let thread = thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while !flag.load(Ordering::SeqCst) {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => sink.lock().unwrap().extend_from_slice(&chunk[..n]),
                    Err(_) => {}
                }
            }
        });
        Self {
            received,
            stop,
            thread: Some(thread),
        }
    }

    fn has(&self, text: &str) -> bool {
        contains(&self.received.lock().unwrap(), text.as_bytes())
    }

    fn finish(mut self) -> Vec<u8> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
        let bytes = self.received.lock().unwrap().clone();
        bytes
    }
}

#[test]
fn no_secret_is_in_any_file_response_event_log_or_metric() {
    let dir = tempfile::tempdir().unwrap();
    let access_log = dir.path().join("access.log");
    let running = start_ok_with_args(
        dir.path(),
        &[],
        &["--access-log", access_log.to_str().unwrap()],
    );
    let mut console = printed(&running);
    let bootstrap = token_in(&console).expect("the bootstrap token is printed");

    // Listen to the live events before anything happens.
    let ticket = json_of(call(
        &running,
        &bootstrap,
        "POST",
        "/api/ws-ticket",
        Some(json!({})),
    ))["ticket"]
        .as_str()
        .unwrap()
        .to_string();
    let listener = Listener::open(&running, &ticket);
    assert!(
        wait_until(|| listener.has("hello")),
        "the socket never opened"
    );

    // Everything that makes or carries a secret.
    let created = json_of(call(
        &running,
        &bootstrap,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "ops", "role": "admin" })),
    ));
    let ops = created["data"]["secret"].as_str().unwrap().to_string();
    let viewer_token = json_of(call(
        &running,
        &bootstrap,
        "POST",
        "/api/tokens",
        Some(json!({ "name": "watcher", "role": "viewer" })),
    ))["data"]["secret"]
        .as_str()
        .unwrap()
        .to_string();
    let subsystem = json_of(call(
        &running,
        &bootstrap,
        "POST",
        "/api/subsystems",
        Some(json!({ "name": "shop" })),
    ))["data"]["id"]
        .as_i64()
        .unwrap();
    let generated = json_of(call(
        &running,
        &ops,
        "PUT",
        &format!("/api/subsystems/{subsystem}/maintenance"),
        Some(json!({ "bypass": { "header": "X-Bypass" } })),
    ))["bypassSecret"]
        .as_str()
        .expect("a generated bypass secret")
        .to_string();
    let chosen = "an-operator-chosen-bypass-secret-0123456789";
    json_of(call(
        &running,
        &ops,
        "PUT",
        "/api/maintenance/global",
        Some(json!({ "bypass": { "header": "X-Bypass", "secret": chosen } })),
    ));
    let issued = TestCa::new("Hygiene CA").issue(&LeafSpec::new(&["shop.example.com"]));
    json_of(call(
        &running,
        &ops,
        "POST",
        "/api/certificates",
        Some(json!({ "name": "shop", "certPem": issued.cert_pem, "keyPem": issued.key_pem })),
    ));
    // A refused attempt and a read by the viewer, for the audit log and the log lines.
    assert_eq!(
        call(&running, "fp_1.wrong-guess", "GET", "/api/status", None).0,
        401
    );
    json_of(call(&running, &viewer_token, "GET", "/api/status", None));

    // The key's PEM body (without the armour) and its marker must not appear.
    let key_body: Vec<String> = issued
        .key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .map(str::to_string)
        .collect();
    let mut secrets: Vec<(&str, String)> = vec![
        ("the ops token secret", secret_of(&ops)),
        ("the viewer token secret", secret_of(&viewer_token)),
        ("the generated bypass secret", generated.clone()),
        ("the chosen bypass secret", chosen.to_string()),
        ("the websocket ticket", ticket.clone()),
        ("the private key marker", "PRIVATE KEY".to_string()),
    ];
    for line in &key_body {
        secrets.push(("the private key", line.clone()));
    }
    let bootstrap_secret = secret_of(&bootstrap);

    // What the API says now: every read, every revision, the audit log, the metrics.
    let mut texts: Vec<(String, String)> = Vec::new();
    let mut read = |path: &str| {
        let (status, text) = call(&running, &bootstrap, "GET", path, None);
        assert_eq!(status, 200, "{path}: {text}");
        texts.push((path.to_string(), text.clone()));
        text
    };
    for path in [
        "/api/status",
        "/api/config",
        "/api/subsystems",
        "/api/tokens",
        "/api/maintenance",
        "/api/audit?limit=200",
        "/api/certificates",
        "/api/ca-bundles",
        "/api/tls",
        "/api/whoami",
    ] {
        read(path);
    }
    read(&format!("/api/subsystems/{subsystem}"));
    let revisions: Value = serde_json::from_str(&read("/api/revisions?limit=100")).unwrap();
    for revision in revisions["data"].as_array().unwrap() {
        read(&format!("/api/revisions/{}", revision["version"]));
    }
    let metrics = {
        let (status, text) = http_with_headers(
            running.admin,
            "GET",
            "/metrics",
            "localhost",
            &[("Authorization", &format!("Bearer {bootstrap}"))],
            None,
        );
        assert_eq!(status, 200);
        text
    };
    texts.push(("/metrics".into(), metrics));
    for (path, text) in &texts {
        for (what, secret) in &secrets {
            if *what == "the private key marker" && path.starts_with("/api/certificates") {
                continue; // a certificate's own PEM is public and says CERTIFICATE, never PRIVATE KEY
            }
            assert!(
                !text.contains(secret.as_str()),
                "{what} is in the response to {path}"
            );
        }
        assert!(
            !text.contains(&bootstrap_secret),
            "the bootstrap secret is in {path}"
        );
    }

    // The events the socket carried: tokens by name, never by secret.
    let events = listener.finish();
    assert!(
        contains(&events, b"tokenChanged"),
        "the socket saw no token event"
    );
    for (what, secret) in &secrets {
        assert!(
            !contains(&events, secret.as_bytes()),
            "{what} went out as an event"
        );
    }
    assert!(!contains(&events, bootstrap_secret.as_bytes()));

    // The log lines (console) and the access log, once they have caught up.
    assert!(wait_until(|| {
        std::fs::read_to_string(&access_log).is_ok_and(|log| log.contains("/api/status"))
    }));
    console.extend(printed(&running));
    let log = std::fs::read_to_string(&access_log).unwrap();
    assert!(
        log.contains("actor="),
        "the access log names the actor: {log}"
    );
    for (what, secret) in &secrets {
        assert!(
            !log.contains(secret.as_str()),
            "{what} is in the access log"
        );
        assert!(
            !console.iter().any(|line| line.contains(secret.as_str())),
            "{what} is in a log line"
        );
    }
    assert!(
        !log.contains(&bootstrap_secret),
        "the bootstrap secret is in the access log"
    );
    assert!(
        console
            .iter()
            .filter(|line| line.contains(&bootstrap_secret))
            .count()
            <= 1,
        "the bootstrap token is printed once, and only on the console"
    );
    assert!(
        !log.contains("ticket="),
        "the ticket stays out of the logged address"
    );

    // Every file the proxy wrote: the database, its write-ahead log, the key file.
    let files = files_under(dir.path());
    // Recent writes may still sit in the write-ahead log, so count it with the database.
    let stored: usize = files
        .iter()
        .filter(|(name, _)| name.contains("fast-proxy.db"))
        .map(|(_, bytes)| bytes.len())
        .sum();
    assert!(
        stored > 4096,
        "an empty database proves nothing ({stored} bytes)"
    );
    for (name, bytes) in &files {
        if name.ends_with("access.log") {
            continue; // checked as text above
        }
        for (what, secret) in &secrets {
            assert!(!contains(bytes, secret.as_bytes()), "{what} is in {name}");
        }
        assert!(
            !contains(bytes, bootstrap_secret.as_bytes()),
            "the bootstrap secret is in {name}"
        );
    }
}
