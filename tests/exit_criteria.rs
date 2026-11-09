//! End-to-end tests of the real `fast-proxy` binary, following the M1 exit
//! criteria: start with an empty database, configure through the admin API,
//! see traffic routed immediately, edit live, and survive a restart.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A backend answering every request with `<name> <request line path>`.
fn start_backend(name: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || answer(stream, name));
        }
    });
    port
}

fn answer(mut stream: TcpStream, name: &str) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.split_whitespace().nth(1).unwrap_or("?");
    let body = format!("{name} {path}");
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// A backend that waits `delay` before answering.
#[cfg(unix)]
fn start_slow_backend(name: &'static str, delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || {
                thread::sleep(delay);
                answer(stream, name);
            });
        }
    });
    port
}

struct Proxy {
    child: Child,
    data: SocketAddr,
    admin: SocketAddr,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn write_config(dir: &Path, data_port: u16, admin_port: u16, extra: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        format!(
            "[data_plane]\nstate_dir = {state:?}\n{extra}\n[[data_plane.listener]]\nname = \"http\"\nlisten = \"127.0.0.1:{data_port}\"\n\n[admin]\nlisten = \"127.0.0.1:{admin_port}\"\n",
            state = dir.join("state")
        ),
    )
    .unwrap();
    path
}

fn spawn(config: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(config)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn start_proxy(dir: &Path) -> Proxy {
    let (data_port, admin_port) = (free_port(), free_port());
    start_proxy_on(dir, data_port, admin_port)
}

fn start_proxy_on(dir: &Path, data_port: u16, admin_port: u16) -> Proxy {
    start_proxy_with(dir, data_port, admin_port, "")
}

fn start_proxy_with(dir: &Path, data_port: u16, admin_port: u16, extra_config: &str) -> Proxy {
    let config = write_config(dir, data_port, admin_port, extra_config);
    let proxy = Proxy {
        child: spawn(&config),
        data: ([127, 0, 0, 1], data_port).into(),
        admin: ([127, 0, 0, 1], admin_port).into(),
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if TcpStream::connect(proxy.admin).is_ok() && TcpStream::connect(proxy.data).is_ok() {
            return proxy;
        }
        thread::sleep(Duration::from_millis(50));
    }
    panic!("the proxy did not start listening in time");
}

/// One request over a fresh connection; returns the status and body.
fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    host: &str,
    body: Option<&Value>,
) -> (u16, String) {
    let payload = body.map(Value::to_string).unwrap_or_default();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    )
    .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap_or((&raw, ""));
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, body.to_string())
}

fn admin(proxy: &Proxy, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let (status, text) = http(proxy.admin, method, path, "admin", body.as_ref());
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn through_proxy(proxy: &Proxy, host: &str, path: &str) -> (u16, String) {
    http(proxy.data, "GET", path, host, None)
}

/// Creates a subsystem answering to `host` whose `/` route goes to `backend_port`.
fn create_site(proxy: &Proxy, name: &str, host: &str, backend_port: u16) -> (i64, i64) {
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
    let (status, body) = admin(
        proxy,
        "POST",
        &format!("{base}/upstreams"),
        Some(json!({ "name": "backend", "addr": format!("127.0.0.1:{backend_port}") })),
    );
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

#[test]
fn an_empty_database_starts_and_routes_nothing_until_configured() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    assert!(dir.path().join("state").join("fast-proxy.db").exists());

    let (status, body) = admin(&proxy, "GET", "/api/subsystems", None);
    assert_eq!(status, 200);
    assert_eq!(body["data"], json!([]));
    assert_eq!(body["version"], 0);

    let (status, text) = through_proxy(&proxy, "shop.example.com", "/");
    assert_eq!(status, 404);
    assert_eq!(text, "unknown host\n");
}

#[test]
fn two_subsystems_route_to_their_own_backends_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let shop_backend = start_backend("shop-backend");
    let blog_backend = start_backend("blog-backend");

    create_site(&proxy, "shop", "shop.example.com", shop_backend);
    create_site(&proxy, "blog", "blog.example.com", blog_backend);

    assert_eq!(
        through_proxy(&proxy, "shop.example.com", "/cart?x=1"),
        (200, "shop-backend /cart?x=1".to_string())
    );
    assert_eq!(
        through_proxy(&proxy, "blog.example.com", "/posts"),
        (200, "blog-backend /posts".to_string())
    );
    assert_eq!(through_proxy(&proxy, "other.example.com", "/").0, 404);
}

#[test]
fn edits_apply_live_without_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let first = start_backend("first");
    let second = start_backend("second");
    let (id, upstream) = create_site(&proxy, "shop", "shop.example.com", first);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").1, "first /");

    // Repoint the upstream.
    let (status, _) = admin(
        &proxy,
        "PUT",
        &format!("/api/subsystems/{id}/upstreams/{upstream}"),
        Some(json!({ "name": "backend", "addr": format!("127.0.0.1:{second}") })),
    );
    assert_eq!(status, 200);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").1, "second /");

    // Disable, then delete the subsystem.
    admin(
        &proxy,
        "PUT",
        &format!("/api/subsystems/{id}"),
        Some(json!({ "name": "shop", "enabled": false })),
    );
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 503);
    admin(&proxy, "DELETE", &format!("/api/subsystems/{id}"), None);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 404);
}

#[test]
fn rolling_back_restores_the_earlier_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("restored");
    let (id, _) = create_site(&proxy, "shop", "shop.example.com", backend);
    assert_eq!(
        through_proxy(&proxy, "shop.example.com", "/").1,
        "restored /"
    );

    admin(&proxy, "DELETE", &format!("/api/subsystems/{id}"), None);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 404);

    let (status, body) = admin(&proxy, "POST", "/api/revisions/4/rollback", Some(json!({})));
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        through_proxy(&proxy, "shop.example.com", "/").1,
        "restored /"
    );

    let (_, status_body) = admin(&proxy, "GET", "/api/status", None);
    assert_eq!(status_body["version"], 6);
    assert_eq!(status_body["data"]["subsystems"]["total"], 1);
}

#[test]
fn the_configuration_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_backend("persistent");
    let (data_port, admin_port) = (free_port(), free_port());
    {
        let proxy = start_proxy_on(dir.path(), data_port, admin_port);
        create_site(&proxy, "shop", "shop.example.com", backend);
        assert_eq!(
            through_proxy(&proxy, "shop.example.com", "/").1,
            "persistent /"
        );
    } // dropped: the process is killed

    let proxy = start_proxy_on(dir.path(), data_port, admin_port);
    assert_eq!(
        through_proxy(&proxy, "shop.example.com", "/").1,
        "persistent /"
    );
    let (_, body) = admin(&proxy, "GET", "/api/subsystems", None);
    assert_eq!(body["data"][0]["name"], "shop");
    assert_eq!(body["version"], 4);
}

#[test]
fn a_port_already_in_use_aborts_startup_with_a_clear_error() {
    let dir = tempfile::tempdir().unwrap();
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let data_port = taken.local_addr().unwrap().port();
    let config = write_config(dir.path(), data_port, free_port(), "");

    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot bind data plane listener \"http\""),
        "{stderr}"
    );
}

#[test]
fn an_invalid_trusted_proxy_aborts_startup() {
    let dir = tempfile::tempdir().unwrap();
    let config = write_config(
        dir.path(),
        free_port(),
        free_port(),
        "trusted_proxies = [\"not-a-cidr\"]",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(&config)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("invalid trusted proxy \"not-a-cidr\""),
        "{stderr}"
    );
}

// ---- shutdown -----------------------------------------------------------
//
// These deliver a real SIGINT, so they run on Unix only. Graceful draining is
// also covered on every platform by the data plane's own drain tests, which
// need no signals; what Windows lacks is an end-to-end test of the console
// Ctrl-C path through the binary.

/// Sends SIGINT, the same as Ctrl-C.
#[cfg(unix)]
fn interrupt(proxy: &Proxy) {
    let status = Command::new("kill")
        .args(["-INT", &proxy.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Waits for the process to exit on its own, returning whether it did so
/// successfully, or `None` if it was still running at the deadline.
#[cfg(unix)]
fn wait_for_exit(proxy: &mut Proxy, within: Duration) -> Option<bool> {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if let Some(status) = proxy.child.try_wait().unwrap() {
            return Some(status.success());
        }
        thread::sleep(Duration::from_millis(25));
    }
    None
}

/// Sends a request on a thread and returns what the client ended up with.
#[cfg(unix)]
fn request_in_background(proxy: &Proxy) -> thread::JoinHandle<Option<(u16, String)>> {
    let addr = proxy.data;
    thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).ok()?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .ok()?;
        write!(
            stream,
            "GET /slow HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n"
        )
        .ok()?;
        let mut raw = String::new();
        stream.read_to_string(&mut raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        Some((
            head.split_whitespace().nth(1)?.parse().ok()?,
            body.to_string(),
        ))
    })
}

#[test]
#[cfg(unix)]
fn ctrl_c_lets_an_in_flight_request_finish_and_then_exits() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = start_proxy(dir.path());
    let backend = start_slow_backend("slow", Duration::from_millis(800));
    create_site(&proxy, "shop", "shop.example.com", backend);

    let client = request_in_background(&proxy);
    thread::sleep(Duration::from_millis(250));
    interrupt(&proxy);

    assert_eq!(
        client.join().unwrap(),
        Some((200, "slow /slow".to_string()))
    );
    assert_eq!(
        wait_for_exit(&mut proxy, Duration::from_secs(5)),
        Some(true)
    );
}

#[test]
#[cfg(unix)]
fn a_request_that_outlives_the_grace_period_is_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = start_proxy_with(
        dir.path(),
        free_port(),
        free_port(),
        "shutdown_grace = \"300ms\"",
    );
    let backend = start_slow_backend("never", Duration::from_secs(20));
    create_site(&proxy, "shop", "shop.example.com", backend);

    let client = request_in_background(&proxy);
    thread::sleep(Duration::from_millis(250));
    let interrupted_at = Instant::now();
    interrupt(&proxy);

    assert_eq!(
        wait_for_exit(&mut proxy, Duration::from_secs(5)),
        Some(true)
    );
    assert!(interrupted_at.elapsed() < Duration::from_secs(4));
    assert_eq!(
        client.join().unwrap(),
        None,
        "the client should get no response"
    );
}

#[test]
#[cfg(unix)]
/// Guards against an open UI connection keeping the process alive. (Upgraded
/// sockets are handed off by hyper, so graceful shutdown does not wait for
/// them; this pins that behavior down.)
fn an_open_admin_websocket_does_not_block_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = start_proxy(dir.path());

    let mut socket = TcpStream::connect(proxy.admin).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        socket,
        "GET /ws HTTP/1.1\r\nHost: admin\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    )
    .unwrap();
    let mut buf = [0u8; 512];
    let n = socket.read(&mut buf).unwrap();
    assert!(
        String::from_utf8_lossy(&buf[..n]).contains("101"),
        "the socket should be open"
    );

    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, Duration::from_secs(5)),
        Some(true)
    );
}
