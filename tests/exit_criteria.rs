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
    let config = write_config(dir, data_port, admin_port, "");
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
