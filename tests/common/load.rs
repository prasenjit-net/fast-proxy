//! A keep-alive load client and what it records, shared by the tests that put
//! the real proxy under traffic while something changes.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One request a client made, with when it ran and what came back.
pub struct Sample {
    pub path: &'static str,
    pub started: Instant,
    pub finished: Instant,
    pub outcome: Result<(u16, String), String>,
}

/// A client that keeps one connection open and reconnects only if it breaks.
pub struct KeepAliveClient {
    addr: SocketAddr,
    host: &'static str,
    stream: Option<TcpStream>,
}

impl KeepAliveClient {
    pub fn new(addr: SocketAddr, host: &'static str) -> Self {
        Self {
            addr,
            host,
            stream: None,
        }
    }

    pub fn get(&mut self, path: &str) -> Result<(u16, String), String> {
        if self.stream.is_none() {
            let stream = TcpStream::connect(self.addr).map_err(|e| format!("connect: {e}"))?;
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
            // Without this, Nagle's algorithm and delayed ACKs can stall each
            // request by tens of milliseconds on Linux.
            stream.set_nodelay(true).ok();
            self.stream = Some(stream);
        }
        let result = Self::exchange(
            self.stream.as_mut().unwrap_or_else(|| unreachable!()),
            self.host,
            path,
        );
        if result.is_err() {
            self.stream = None;
        }
        result
    }

    fn exchange(stream: &mut TcpStream, host: &str, path: &str) -> Result<(u16, String), String> {
        // One write per request: `write!` on a bare socket would split it into
        // several small packets.
        let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .map_err(|e| format!("send: {e}"))?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        let head_end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            match stream.read(&mut chunk) {
                Ok(0) => return Err("connection closed before the response head".into()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(format!("read head: {e}")),
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| format!("bad status line: {head:?}"))?;
        let length: usize = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().ok())
            })
            .flatten()
            .ok_or_else(|| format!("no content-length in {head:?}"))?;
        while buf.len() < head_end + length {
            match stream.read(&mut chunk) {
                Ok(0) => return Err("connection closed mid-body".into()),
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(format!("read body: {e}")),
            }
        }
        Ok((
            status,
            String::from_utf8_lossy(&buf[head_end..head_end + length]).to_string(),
        ))
    }
}

/// Sends `path` over one keep-alive connection until `stop` is set.
pub fn client_loop(
    addr: SocketAddr,
    host: &'static str,
    path: &'static str,
    stop: Arc<AtomicBool>,
) -> Vec<Sample> {
    let mut client = KeepAliveClient::new(addr, host);
    let mut samples = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let outcome = client.get(path);
        samples.push(Sample {
            path,
            started,
            finished: Instant::now(),
            outcome,
        });
    }
    samples
}
