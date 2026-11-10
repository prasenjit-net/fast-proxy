//! Helpers shared by the data plane's end-to-end tests: config builders, a
//! running proxy harness, and a raw-TCP client.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    Config, EndpointAddr, EndpointConfig, EndpointId, EndpointState, GroupPolicy, HostPattern,
    RetryPolicy, RouteConfig, RouteId, RoutePrefix, SubsystemConfig, SubsystemId, SubsystemName,
    UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix,
};
use fast_proxy_routing::{compile, SnapshotHolder};
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::{
    bind, DataPlane, Forward, ListenerSpec, Observability, RequestPipeline, TrustedProxies,
    UpstreamClient,
};

/// A route with default settings (no rewriting, host preserved).
pub fn route(id: i64, prefix: &str) -> RouteConfig {
    RouteConfig {
        id: RouteId(id),
        prefix: RoutePrefix::parse(prefix).unwrap(),
        upstream_group_id: UpstreamGroupId(1),
        strip_prefix: false,
        upstream_prefix: UpstreamPrefix::default(),
        preserve_host: true,
        connect_timeout: Duration::from_secs(5),
        response_timeout: Duration::from_secs(30),
        retry: RetryPolicy::default(),
        enabled: true,
    }
}

/// An enabled subsystem answering to `host`, whose single upstream is
/// `endpoint`. Routes point at that upstream whatever id they were built with.
pub fn site_to(
    id: i64,
    name: &str,
    host: &str,
    endpoint: SocketAddr,
    routes: Vec<RouteConfig>,
) -> SubsystemConfig {
    site_to_group(id, name, host, &[endpoint], GroupPolicy::default(), routes)
}

/// Like [`site_to`] with several enabled endpoints (ids 1, 2, ...) under
/// `policy`.
pub fn site_to_group(
    id: i64,
    name: &str,
    host: &str,
    endpoints: &[SocketAddr],
    policy: GroupPolicy,
    routes: Vec<RouteConfig>,
) -> SubsystemConfig {
    SubsystemConfig {
        id: SubsystemId(id),
        name: SubsystemName::parse(name).unwrap(),
        enabled: true,
        description: String::new(),
        https: fast_proxy_model::HttpsPolicy::default(),
        hosts: vec![HostPattern::parse(host).unwrap()],
        upstream_groups: vec![UpstreamGroupConfig {
            id: UpstreamGroupId(1),
            name: "backend".into(),
            policy,
            endpoints: endpoints
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
        routes,
    }
}

/// A proxy on an ephemeral port serving a compiled config.
pub struct Harness {
    pub plane: DataPlane,
    pub holder: Arc<SnapshotHolder>,
    pub addr: SocketAddr,
    pub shutdown: CancellationToken,
    pub metrics: Arc<Metrics>,
    pub events: EndpointEvents,
    pub access_log: Arc<AccessLog>,
}

impl Harness {
    /// Serves `config` with a custom forwarder (no real upstream).
    pub async fn start<F: Forward>(config: Config, forwarder: F) -> Self {
        Self::start_full(config, |_, _| forwarder, None).await
    }

    /// Serves `config` through the real upstream client.
    pub async fn proxying(config: Config, trusted_proxies: &[&str]) -> Self {
        Self::proxying_with_log(config, trusted_proxies, None).await
    }

    /// Like [`Harness::proxying`], also writing the access log to `log_path`.
    pub async fn proxying_with_log(
        config: Config,
        trusted_proxies: &[&str],
        log_path: Option<&Path>,
    ) -> Self {
        let entries: Vec<String> = trusted_proxies.iter().map(ToString::to_string).collect();
        let trusted = TrustedProxies::parse(&entries).unwrap();
        Self::start_full(
            config,
            |metrics, events| UpstreamClient::new(trusted, metrics, events),
            log_path,
        )
        .await
    }

    /// The general form: `make_forwarder` receives the metrics and the endpoint
    /// event bus the pipeline reports to.
    pub async fn start_full<F: Forward>(
        config: Config,
        make_forwarder: impl FnOnce(Arc<Metrics>, EndpointEvents) -> F,
        log_path: Option<&Path>,
    ) -> Self {
        let holder = Arc::new(SnapshotHolder::new());
        holder.swap(compile(1, &config, &RuntimeRegistry::new()).unwrap());
        let specs = [ListenerSpec {
            name: "http".into(),
            addr: ([127, 0, 0, 1], 0).into(),
        }];
        let bound = bind(&specs).await.unwrap();
        let shutdown = CancellationToken::new();
        let metrics = Arc::new(Metrics::new());
        let events = EndpointEvents::new();
        let access_log = Arc::new(AccessLog::open(log_path).await);
        let observability = Observability {
            metrics: metrics.clone(),
            access_log: access_log.clone(),
        };
        let pipeline = Arc::new(RequestPipeline::new(
            holder.clone(),
            make_forwarder(metrics.clone(), events.clone()),
            observability,
        ));
        let plane = DataPlane::start(bound, pipeline, shutdown.clone());
        let addr = plane.local_addrs()[0].1;
        Self {
            plane,
            holder,
            addr,
            shutdown,
            metrics,
            events,
            access_log,
        }
    }
}

/// A parsed HTTP/1.1 response.
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Parses a complete response (head and body) read until EOF.
pub fn parse_reply(bytes: Vec<u8>) -> Reply {
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap();
            (key.to_string(), value.trim().to_string())
        })
        .collect();
    Reply {
        status,
        headers,
        body: body.to_string(),
    }
}

/// Sends `raw` and reads until the server closes the connection.
pub async fn send_raw(addr: SocketAddr, raw: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    parse_reply(bytes)
}

/// A `GET` with `Connection: close`; `extra` is raw header lines, each ending in CRLF.
pub async fn get(addr: SocketAddr, host: &str, path: &str, extra: &str) -> Reply {
    send_raw(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

/// Polls `condition` for up to a second.
pub async fn eventually(condition: impl FnMut() -> bool) -> bool {
    eventually_within(Duration::from_secs(1), condition).await
}

/// Polls `condition` for up to `limit`, for waits that depend on the platform
/// (connecting to a closed port takes about a second on Windows).
pub async fn eventually_within(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        if condition() {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
