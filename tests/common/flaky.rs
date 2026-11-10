//! A backend that can fail on demand (plan task Q1): killed and revived on the
//! same port, answering a chosen status, slow, with its hits counted. Built on
//! std threads and no Unix-only commands, so it runs on Windows too.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// The path health probes use in these tests; probes are counted apart from
/// ordinary requests and can fail independently of them.
pub const HEALTH_PATH: &str = "/healthz";

struct State {
    name: &'static str,
    /// 0 answers 200; anything else is answered for ordinary requests.
    status: AtomicU16,
    /// 0 answers 200; anything else is answered for health probes.
    health_status: AtomicU16,
    delay_ms: AtomicU64,
    hits: AtomicUsize,
    probes: AtomicUsize,
}

pub struct FlakyBackend {
    pub port: u16,
    state: Arc<State>,
    stop: Mutex<Arc<AtomicBool>>,
}

impl FlakyBackend {
    pub fn start(name: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let backend = Self {
            port: listener.local_addr().unwrap().port(),
            state: Arc::new(State {
                name,
                status: AtomicU16::new(0),
                health_status: AtomicU16::new(0),
                delay_ms: AtomicU64::new(0),
                hits: AtomicUsize::new(0),
                probes: AtomicUsize::new(0),
            }),
            stop: Mutex::new(Arc::new(AtomicBool::new(false))),
        };
        backend.serve(listener);
        backend
    }

    fn addr(&self) -> SocketAddr {
        ([127, 0, 0, 1], self.port).into()
    }

    fn serve(&self, listener: TcpListener) {
        let stop = Arc::new(AtomicBool::new(false));
        *self.stop.lock().unwrap() = stop.clone();
        let state = self.state.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if stop.load(Ordering::SeqCst) {
                    return;
                }
                let Ok(stream) = stream else { return };
                let state = state.clone();
                thread::spawn(move || answer(stream, &state));
            }
        });
    }

    /// Simulates a crash: stops listening, so new connections are refused.
    /// Returns once the port really refuses.
    pub fn kill(&self) {
        self.stop.lock().unwrap().store(true, Ordering::SeqCst);
        // Wake the accept loop so it notices and drops the listener.
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if TcpStream::connect_timeout(&self.addr(), Duration::from_secs(2)).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the killed backend still accepts connections");
    }

    /// Brings a killed backend back on the same port.
    pub fn revive(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(listener) = TcpListener::bind(self.addr()) {
                self.serve(listener);
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("the killed backend's port never became free");
    }

    /// Answers every ordinary request with `status` until [`FlakyBackend::recover`].
    pub fn fail_with(&self, status: u16) {
        self.state.status.store(status, Ordering::SeqCst);
    }

    pub fn recover(&self) {
        self.state.status.store(0, Ordering::SeqCst);
    }

    /// Answers health probes (only) with `status`; 0 goes back to 200.
    pub fn set_health_status(&self, status: u16) {
        self.state.health_status.store(status, Ordering::SeqCst);
    }

    /// Waits this long before answering ordinary requests.
    pub fn set_delay(&self, delay: Duration) {
        let millis = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
        self.state.delay_ms.store(millis, Ordering::SeqCst);
    }

    /// Ordinary requests received (health probes excluded).
    pub fn hits(&self) -> usize {
        self.state.hits.load(Ordering::SeqCst)
    }

    /// Health probes received.
    pub fn probes(&self) -> usize {
        self.state.probes.load(Ordering::SeqCst)
    }
}

fn answer(mut stream: TcpStream, state: &State) {
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).unwrap_or(0);
    let request = String::from_utf8_lossy(&buf[..n]);
    let path = request.split_whitespace().nth(1).unwrap_or("?");
    let (status, delay) = if path == HEALTH_PATH {
        state.probes.fetch_add(1, Ordering::SeqCst);
        (state.health_status.load(Ordering::SeqCst), 0)
    } else {
        state.hits.fetch_add(1, Ordering::SeqCst);
        (
            state.status.load(Ordering::SeqCst),
            state.delay_ms.load(Ordering::SeqCst),
        )
    };
    if delay > 0 {
        thread::sleep(Duration::from_millis(delay));
    }
    let status = if status == 0 { 200 } else { status };
    let body = format!("{} {path}", state.name);
    let _ = write!(
        stream,
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}
