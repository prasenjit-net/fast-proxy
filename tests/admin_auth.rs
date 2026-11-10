//! The admin plane's authentication against the real binary: a fresh install
//! prints a bootstrap token once and is closed without it, roles are enforced,
//! the `Host` check holds, and exposing the plane unprotected is refused.

mod common;

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use common::{free_port, http_with_headers, write_config};
use serde_json::{json, Value};

const TOKEN_MARKER: &str = "admin token (shown once, store it now): ";

/// A proxy started with authentication required and its console output.
struct Running {
    child: Child,
    admin: std::net::SocketAddr,
    lines: mpsc::Receiver<String>,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start(dir: &Path, env: &[(&str, &str)], extra_args: &[&str]) -> Result<Running, String> {
    let data_port = free_port();
    let admin_port = free_port();
    let config = write_config(dir, data_port, admin_port, "");
    let mut command = Command::new(env!("CARGO_BIN_EXE_fast-proxy"));
    command
        .arg("--config")
        .arg(&config)
        .args(["--admin-auth", "required"])
        .args(extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, lines) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let admin: std::net::SocketAddr = ([127, 0, 0, 1], admin_port).into();
    let mut running = Running {
        child,
        admin,
        lines,
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = running.child.try_wait().unwrap() {
            return Err(format!("exited at startup ({status})"));
        }
        if std::net::TcpStream::connect(admin).is_ok() {
            return Ok(running);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("did not start listening".into())
}

fn start_ok(dir: &Path, env: &[(&str, &str)]) -> Running {
    for _ in 0..5 {
        if let Ok(running) = start(dir, env, &[]) {
            return running;
        }
    }
    panic!("the proxy would not start");
}

/// The console lines printed so far.
fn printed(running: &Running) -> Vec<String> {
    thread::sleep(Duration::from_millis(200));
    running.lines.try_iter().collect()
}

fn token_in(lines: &[String]) -> Option<String> {
    lines
        .iter()
        .find_map(|line| line.strip_prefix(TOKEN_MARKER))
        .map(str::to_string)
}

fn call(
    running: &Running,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (u16, Value) {
    let authorization = token.map(|t| format!("Bearer {t}"));
    let headers: Vec<(&str, &str)> = authorization
        .as_deref()
        .map(|value| vec![("Authorization", value)])
        .unwrap_or_default();
    let (status, text) = http_with_headers(
        running.admin,
        method,
        path,
        "localhost",
        &headers,
        body.as_ref(),
    );
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[test]
fn a_fresh_install_prints_a_bootstrap_token_once_and_is_closed_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let first = start_ok(dir.path(), &[]);
    let token = token_in(&printed(&first)).expect("the bootstrap token is printed");
    assert!(token.starts_with("fp_"));

    assert_eq!(call(&first, "GET", "/api/status", None, None).0, 401);
    assert_eq!(
        call(&first, "GET", "/api/status", Some("fp_1.wrong"), None).0,
        401
    );
    assert_eq!(
        call(&first, "GET", "/api/status", Some(&token), None).0,
        200
    );
    assert_eq!(
        call(&first, "GET", "/api/health", None, None).0,
        200,
        "health stays open"
    );
    drop(first);

    // A second start finds an admin token and prints nothing; the old one works.
    let second = start_ok(dir.path(), &[]);
    let lines = printed(&second);
    assert!(token_in(&lines).is_none(), "printed again: {lines:?}");
    assert_eq!(
        call(&second, "GET", "/api/status", Some(&token), None).0,
        200
    );
}

#[test]
fn the_bootstrap_secret_is_in_no_api_response_or_metric() {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok(dir.path(), &[]);
    let token = token_in(&printed(&running)).unwrap();
    let secret = token.rsplit('.').next().unwrap();
    let (_, list) = call(&running, "GET", "/api/tokens", Some(&token), None);
    assert!(!list.to_string().contains(secret));
    let (_, revisions) = call(&running, "GET", "/api/revisions", Some(&token), None);
    assert!(!revisions.to_string().contains(secret));
    let (_, metrics) = http_text(&running, "/metrics", &token);
    assert!(!metrics.contains(secret));
}

fn http_text(running: &Running, path: &str, token: &str) -> (u16, String) {
    http_with_headers(
        running.admin,
        "GET",
        path,
        "localhost",
        &[("Authorization", &format!("Bearer {token}"))],
        None,
    )
}

#[test]
fn a_seeded_token_is_used_and_nothing_is_printed() {
    let dir = tempfile::tempdir().unwrap();
    let seed = format!("fp_1.{}", "z".repeat(43));
    let running = start_ok(dir.path(), &[("FAST_PROXY_BOOTSTRAP_TOKEN", &seed)]);
    let lines = printed(&running);
    assert!(token_in(&lines).is_none(), "{lines:?}");
    assert_eq!(
        call(&running, "GET", "/api/status", Some(&seed), None).0,
        200
    );
}

#[test]
fn a_weak_seed_stops_startup_with_a_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(write_config(dir.path(), free_port(), free_port(), ""))
        .args(["--admin-auth", "required"])
        .env("FAST_PROXY_BOOTSTRAP_TOKEN", "fp_1.short")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = child.wait().unwrap();
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(!status.success());
    assert!(stderr.contains("FAST_PROXY_BOOTSTRAP_TOKEN"), "{stderr}");
}

#[test]
fn a_viewer_token_reads_but_cannot_write_or_manage_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok(dir.path(), &[]);
    let admin = token_in(&printed(&running)).unwrap();
    let (status, created) = call(
        &running,
        "POST",
        "/api/tokens",
        Some(&admin),
        Some(json!({ "name": "eyes", "role": "viewer" })),
    );
    assert_eq!(status, 201, "{created}");
    let viewer = created["data"]["secret"].as_str().unwrap().to_string();

    assert_eq!(
        call(&running, "GET", "/api/subsystems", Some(&viewer), None).0,
        200
    );
    let write = call(
        &running,
        "POST",
        "/api/subsystems",
        Some(&viewer),
        Some(json!({ "name": "shop" })),
    );
    assert_eq!(write.0, 403);
    assert_eq!(
        call(&running, "GET", "/api/tokens", Some(&viewer), None).0,
        403
    );
    let (_, list) = call(&running, "GET", "/api/subsystems", Some(&admin), None);
    assert_eq!(list["data"], json!([]), "the refused write changed nothing");
}

#[test]
fn a_foreign_host_header_is_refused_over_a_real_socket() {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok(dir.path(), &[]);
    let (status, _) = http_with_headers(
        running.admin,
        "GET",
        "/api/health",
        "evil.example",
        &[],
        None,
    );
    assert_eq!(status, 421);
}

#[test]
fn exposing_the_admin_plane_without_protection_is_refused_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port().to_string();
    let listen = format!("0.0.0.0:{port}");
    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(write_config(dir.path(), free_port(), free_port(), ""))
        .args(["--admin-auth", "required", "--admin-listen", &listen])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("admin.allow_insecure_remote"), "{stderr}");

    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(write_config(dir.path(), free_port(), free_port(), ""))
        .args(["--admin-auth", "off", "--admin-listen", &listen])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("admin.auth"));
}
