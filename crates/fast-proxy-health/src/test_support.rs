//! A flaky in-process backend and snapshot builders for the health tests.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    Config, EndpointAddr, EndpointConfig, EndpointId, EndpointState, GroupPolicy, HealthCheck,
    HealthKind, HostPattern, RetryPolicy, RouteConfig, RouteId, RoutePrefix, SubsystemConfig,
    SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix,
};
use fast_proxy_routing::{compile, Snapshot};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// Answers every HTTP request with a settable status, can be killed and
/// revived on the same port, and records the request lines it saw.
pub struct Backend {
    pub addr: SocketAddr,
    status: AtomicU16,
    pub paths: Mutex<Vec<String>>,
    stop: Mutex<CancellationToken>,
}

impl Backend {
    pub async fn start() -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend = Arc::new(Self {
            addr: listener.local_addr().unwrap(),
            status: AtomicU16::new(200),
            paths: Mutex::new(Vec::new()),
            stop: Mutex::new(CancellationToken::new()),
        });
        backend.serve(listener);
        backend
    }

    fn serve(self: &Arc<Self>, listener: TcpListener) {
        let (state, stop) = (self.clone(), CancellationToken::new());
        *self.stop.lock().unwrap() = stop.clone();
        tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {
                    accepted = listener.accept() => accepted,
                    () = stop.cancelled() => return,
                };
                let Ok((mut stream, _)) = accepted else {
                    return;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    if let Some(path) = text.split_whitespace().nth(1) {
                        state.paths.lock().unwrap().push(path.to_string());
                    }
                    let status = state.status.load(Ordering::SeqCst);
                    let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
    }

    pub fn set_status(&self, status: u16) {
        self.status.store(status, Ordering::SeqCst);
    }

    pub fn kill(&self) {
        self.stop.lock().unwrap().cancel();
    }

    pub async fn revive(self: &Arc<Self>) {
        for _ in 0..100 {
            if let Ok(listener) = TcpListener::bind(self.addr).await {
                self.serve(listener);
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the killed backend's port never became free");
    }

    pub fn probes_of(&self, path: &str) -> usize {
        self.paths
            .lock()
            .unwrap()
            .iter()
            .filter(|p| *p == path)
            .count()
    }
}

/// A fast HTTP check: the fastest interval and thresholds of 2.
pub fn fast_check(path: &str) -> HealthCheck {
    HealthCheck {
        kind: HealthKind::Http,
        path: path.into(),
        interval: Duration::from_millis(100),
        timeout: Duration::from_millis(100),
        rise: 2,
        fall: 2,
        ..HealthCheck::default()
    }
}

/// A subsystem `shop` with one group `backend` at the given addresses.
pub fn config_for(check: HealthCheck, addrs: &[SocketAddr]) -> Config {
    Config {
        subsystems: vec![SubsystemConfig {
            id: SubsystemId(1),
            name: SubsystemName::parse("shop").unwrap(),
            enabled: true,
            description: String::new(),
            https: fast_proxy_model::HttpsPolicy::default(),
            hosts: vec![HostPattern::parse("shop.example.com").unwrap()],
            upstream_groups: vec![UpstreamGroupConfig {
                id: UpstreamGroupId(1),
                name: "backend".into(),
                policy: GroupPolicy {
                    health: check,
                    ..GroupPolicy::default()
                },
                endpoints: addrs
                    .iter()
                    .enumerate()
                    .map(|(index, addr)| EndpointConfig {
                        id: EndpointId(index as i64 + 1),
                        addr: EndpointAddr::parse(&addr.to_string()).unwrap(),
                        weight: 1,
                        state: EndpointState::Enabled,
                    })
                    .collect(),
            }],
            routes: vec![RouteConfig {
                id: RouteId(1),
                prefix: RoutePrefix::parse("/").unwrap(),
                upstream_group_id: UpstreamGroupId(1),
                strip_prefix: false,
                upstream_prefix: UpstreamPrefix::default(),
                preserve_host: true,
                connect_timeout: Duration::from_secs(1),
                response_timeout: Duration::from_secs(1),
                retry: RetryPolicy::default(),
                enabled: true,
            }],
        }],
    }
}

/// Compiles `config`, carrying runtime state over from `previous`.
pub fn snapshot(config: &Config, previous: Option<&Snapshot>) -> Snapshot {
    let registry = previous.map_or_else(RuntimeRegistry::new, Snapshot::runtime_registry);
    compile(1, config, &registry).unwrap()
}

/// Polls `condition` for up to `seconds`.
pub async fn eventually(seconds: u64, mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..seconds * 50 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}
