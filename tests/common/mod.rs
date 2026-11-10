//! Helpers shared by the end-to-end tests that run the real `fast-proxy`
//! binary: free ports, backends, spawning a configured proxy, and a tiny HTTP
//! client for both planes.
#![allow(dead_code)] // each test binary uses a different subset

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub mod flaky;
pub mod groups;
pub mod load;
pub mod tls_stack;

use serde_json::{json, Value};

pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// A backend answering every request with `<name> <request line path>`.
pub fn start_backend(name: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || answer(stream, name));
        }
    });
    port
}

pub fn answer(mut stream: TcpStream, name: &str) {
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
pub fn start_slow_backend(name: &'static str, delay: Duration) -> u16 {
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

pub struct Proxy {
    pub child: Child,
    pub data: SocketAddr,
    pub admin: SocketAddr,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn write_config(
    dir: &Path,
    data_port: u16,
    admin_port: u16,
    extra: &str,
) -> std::path::PathBuf {
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

pub fn spawn(config: &Path) -> Child {
    spawn_with_args(config, &[])
}

/// Spawns the proxy with extra command-line flags after `--config`.
pub fn spawn_with_args(config: &Path, args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(config)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

pub fn start_proxy(dir: &Path) -> Proxy {
    start_on_free_ports(dir, "", "")
}

pub fn start_proxy_on(dir: &Path, data_port: u16, admin_port: u16) -> Proxy {
    start_proxy_with(dir, data_port, admin_port, "")
}

pub fn start_proxy_with(dir: &Path, data_port: u16, admin_port: u16, extra_config: &str) -> Proxy {
    start_proxy_with_tail(dir, data_port, admin_port, extra_config, "")
}

/// Starts on freshly picked ports. A "free" port can be taken by a parallel
/// test (or an outgoing connection) before the proxy binds it, in which case
/// the proxy exits at once; retry with new ports instead of failing.
pub fn start_on_free_ports(dir: &Path, extra_config: &str, tail: &str) -> Proxy {
    let mut last_error = String::new();
    for _ in 0..5 {
        match try_start(dir, free_port(), free_port(), extra_config, tail) {
            Ok(proxy) => return proxy,
            Err(error) => last_error = error,
        }
    }
    panic!("the proxy would not start on any of 5 port pairs: {last_error}");
}

/// Like [`start_proxy_with`], appending `tail` (extra config sections) to the file.
pub fn start_proxy_with_tail(
    dir: &Path,
    data_port: u16,
    admin_port: u16,
    extra_config: &str,
    tail: &str,
) -> Proxy {
    try_start(dir, data_port, admin_port, extra_config, tail)
        .unwrap_or_else(|error| panic!("{error}"))
}

pub fn try_start(
    dir: &Path,
    data_port: u16,
    admin_port: u16,
    extra_config: &str,
    tail: &str,
) -> Result<Proxy, String> {
    try_start_with_args(dir, data_port, admin_port, (extra_config, tail), &[])
}

/// Like [`try_start`], also passing command-line `args`; `config` is the extra
/// `[data_plane]` lines and the tail sections.
pub fn try_start_with_args(
    dir: &Path,
    data_port: u16,
    admin_port: u16,
    (extra_config, tail): (&str, &str),
    args: &[&str],
) -> Result<Proxy, String> {
    let config = write_config(dir, data_port, admin_port, extra_config);
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(tail);
    std::fs::write(&config, text).unwrap();
    let mut proxy = Proxy {
        child: spawn_with_args(&config, args),
        data: ([127, 0, 0, 1], data_port).into(),
        admin: ([127, 0, 0, 1], admin_port).into(),
    };
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if let Some(status) = proxy.child.try_wait().unwrap() {
            return Err(format!("the proxy exited at startup ({status})"));
        }
        if TcpStream::connect(proxy.admin).is_ok() && TcpStream::connect(proxy.data).is_ok() {
            return Ok(proxy);
        }
        thread::sleep(Duration::from_millis(50));
    }
    Err("the proxy did not start listening in time".to_string())
}

/// One request over a fresh connection; returns the status and body.
pub fn http(
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

pub fn admin(proxy: &Proxy, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let (status, text) = http(proxy.admin, method, path, "admin", body.as_ref());
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

pub fn through_proxy(proxy: &Proxy, host: &str, path: &str) -> (u16, String) {
    http(proxy.data, "GET", path, host, None)
}

/// Creates a subsystem answering to `host` whose `/` route goes to `backend_port`.
pub fn create_site(proxy: &Proxy, name: &str, host: &str, backend_port: u16) -> (i64, i64) {
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
        Some(json!({
            "name": "backend",
            "endpoints": [{ "addr": format!("127.0.0.1:{backend_port}") }]
        })),
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

/// Sends SIGINT, the same as Ctrl-C.
#[cfg(unix)]
pub fn interrupt(proxy: &Proxy) {
    let status = Command::new("kill")
        .args(["-INT", &proxy.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}

/// Waits for the process to exit on its own, returning whether it did so
/// successfully, or `None` if it was still running at the deadline.
#[cfg(unix)]
pub fn wait_for_exit(proxy: &mut Proxy, within: Duration) -> Option<bool> {
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
pub fn request_in_background(proxy: &Proxy) -> thread::JoinHandle<Option<(u16, String)>> {
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

/// The value of the first exposition line that starts with `prefix`.
pub fn metric(proxy: &Proxy, prefix: &str) -> Option<f64> {
    let (_, text) = http(proxy.admin, "GET", "/metrics", "admin", None);
    text.lines()
        .find(|line| line.starts_with(prefix))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

pub fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(25));
    }
    false
}

/// A backend that serves many requests per connection (HTTP/1.1 keep-alive),
/// answering each with `name` after `delay`, so that requests stay in flight.
pub fn start_keepalive_backend(name: &'static str, delay: Duration) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            thread::spawn(move || serve_keepalive(stream, name, delay));
        }
    });
    port
}

fn serve_keepalive(mut stream: TcpStream, name: &str, delay: Duration) {
    stream.set_nodelay(true).ok();
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        // One request ends at the blank line (these tests send no bodies).
        while !pending.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
            }
        }
        let end = pending
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(0)
            + 4;
        pending.drain(..end);
        thread::sleep(delay);
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{name}",
            name.len()
        );
        if stream.write_all(response.as_bytes()).is_err() {
            return;
        }
    }
}
