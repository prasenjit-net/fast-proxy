//! A real data plane and config service in one process with an HTTPS listener,
//! for the TLS end-to-end tests: certificates issued by an in-memory CA are
//! uploaded as PEM strings, so no files are involved.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_config_service::{ConfigService, TlsSettings};
use fast_proxy_dataplane::{
    bind, DataPlane, Http2Limits, ListenerSpec, Observability, RequestPipeline, TlsPlane,
    TlsPlaneSettings, TrustedProxies, UpstreamClient,
};
use fast_proxy_model::{
    CertificateId, EndpointAddr, EndpointState, GroupPolicy, HostPattern, HttpsPolicy, RetryPolicy,
    RoutePrefix, SubsystemId, SubsystemName, UpstreamPrefix,
};
use fast_proxy_store::repository::{hosts, routes, subsystems, upstreams};
use fast_proxy_store::{
    EndpointFields, Mutation, RouteFields, Store, SubsystemFields, UpstreamFields,
};
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use fast_proxy_test_pki::{handshake, ClientHello, LeafSpec, TestCa};
use fast_proxy_tls::{MinTlsVersion, ServerTlsPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

pub struct Options {
    pub min_version: MinTlsVersion,
    pub reject_unknown_sni: bool,
    pub handshake_timeout: Duration,
    pub http2: Http2Limits,
    pub offer_http2: bool,
    pub max_handshakes: usize,
    /// TLS on for the process (the HTTPS listener runs). Off: no HTTPS listener.
    pub tls: bool,
    /// The port redirects name; defaults to the HTTPS listener's own.
    pub public_port: Option<u16>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_version: MinTlsVersion::V1_2,
            reject_unknown_sni: false,
            handshake_timeout: Duration::from_secs(10),
            http2: Http2Limits::default(),
            offer_http2: true,
            max_handshakes: 64,
            tls: true,
            public_port: None,
        }
    }
}

pub struct Stack {
    pub service: Arc<ConfigService>,
    pub metrics: Arc<Metrics>,
    pub https: SocketAddr,
    pub http: SocketAddr,
    pub backend: SocketAddr,
    pub shutdown: CancellationToken,
    plane: Option<DataPlane>,
    _dir: tempfile::TempDir,
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// A backend answering every request with the request head it received.
pub async fn echo_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).to_lowercase();
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{head}",
                            head.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    addr
}

impl Stack {
    /// Stops accepting and waits for open connections to finish, at most `grace`.
    pub async fn drain(&mut self, grace: Duration) -> fast_proxy_dataplane::DrainReport {
        self.shutdown.cancel();
        match self.plane.take() {
            Some(plane) => plane.stopped(grace).await,
            None => fast_proxy_dataplane::DrainReport {
                forced_connections: 0,
            },
        }
    }

    pub async fn start(options: Options) -> Self {
        Self::start_with_backend(options, None).await
    }

    /// Like [`Stack::start`], proxying to `backend` instead of the echo backend.
    pub async fn start_with_backend(options: Options, backend: Option<SocketAddr>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let metrics = Arc::new(Metrics::new());
        let store = Store::open(&dir.path().join("fast-proxy.db")).unwrap();
        let settings = TlsSettings {
            enabled: options.tls,
            master_key_file: Some(dir.path().join("master.key")),
            env_master_key: None,
        };
        let service = Arc::new(
            ConfigService::start_with_tls(store, metrics.clone(), settings)
                .await
                .unwrap(),
        );
        let observability = Observability {
            metrics: metrics.clone(),
            access_log: Arc::new(AccessLog::open(None).await),
        };
        let forwarder = UpstreamClient::new(
            TrustedProxies::parse(&[]).unwrap(),
            metrics.clone(),
            EndpointEvents::new(),
        );
        let loopback: SocketAddr = ([127, 0, 0, 1], 0).into();
        let http = bind(&[ListenerSpec {
            name: "http".into(),
            addr: loopback,
        }])
        .await
        .unwrap();
        let https = bind(&[ListenerSpec {
            name: "https".into(),
            addr: loopback,
        }])
        .await
        .unwrap()
        .remove(0);
        let public_port = options
            .public_port
            .unwrap_or_else(|| https.local_addr().port());
        let pipeline = Arc::new(
            RequestPipeline::new(service.snapshot_holder(), forwarder, observability)
                .with_https_port(public_port),
        );
        let tls = options.tls.then(|| {
            TlsPlane::new(
                https,
                service.snapshot_holder(),
                TlsPlaneSettings {
                    policy: ServerTlsPolicy {
                        min_version: options.min_version,
                        http2: options.offer_http2,
                    },
                    handshake_timeout: options.handshake_timeout,
                    max_concurrent_handshakes: options.max_handshakes,
                    reject_unknown_sni: options.reject_unknown_sni,
                    http2: options.http2,
                },
                metrics.clone(),
            )
            .unwrap()
        });
        let shutdown = CancellationToken::new();
        let plane = DataPlane::start_with_tls(http, tls, pipeline, shutdown.clone());
        let addr_of = |name: &str| {
            plane
                .local_addrs()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, addr)| *addr)
                .unwrap()
        };
        let http = addr_of("http");
        let https = if options.tls { addr_of("https") } else { http };
        Self {
            service,
            metrics,
            https,
            http,
            backend: match backend {
                Some(backend) => backend,
                None => echo_backend().await,
            },
            shutdown,
            plane: Some(plane),
            _dir: dir,
        }
    }

    /// A subsystem answering to `host`, routing `/` to the echo backend.
    pub async fn add_site(&self, name: &'static str, host: &'static str) -> SubsystemId {
        let backend = self.backend;
        self.service
            .apply(move |conn| {
                let id = subsystems::create(
                    conn,
                    &SubsystemFields {
                        name: SubsystemName::parse(name).unwrap(),
                        enabled: true,
                        description: String::new(),
                        https: HttpsPolicy::default(),
                    },
                )?;
                hosts::add(conn, id, &HostPattern::parse(host).unwrap())?;
                let group = upstreams::create(
                    conn,
                    id,
                    &UpstreamFields {
                        name: "backend".into(),
                        policy: GroupPolicy::default(),
                    },
                    &[EndpointFields {
                        addr: EndpointAddr::parse(&backend.to_string()).unwrap(),
                        weight: 1,
                        state: EndpointState::Enabled,
                    }],
                )?;
                routes::create(
                    conn,
                    id,
                    &RouteFields {
                        prefix: RoutePrefix::parse("/").unwrap(),
                        upstream_group_id: group,
                        strip_prefix: false,
                        upstream_prefix: UpstreamPrefix::default(),
                        preserve_host: true,
                        connect_timeout: Duration::from_secs(5),
                        response_timeout: Duration::from_secs(30),
                        retry: RetryPolicy::default(),
                        enabled: true,
                    },
                )?;
                Ok(Mutation {
                    value: id,
                    summary: format!("site {name}"),
                })
            })
            .await
            .unwrap()
            .value
    }

    /// Uploads a certificate and turns HTTPS on for `site` with it.
    pub async fn serve_with(
        &self,
        site: SubsystemId,
        name: &str,
        ca: &TestCa,
        names: &[&str],
    ) -> CertificateId {
        let issued = ca.issue(&LeafSpec::new(names));
        let id = self
            .service
            .upload_certificate(name, &issued.chain_pem, &issued.key_pem)
            .await
            .unwrap()
            .value
            .id;
        let https = HttpsPolicy {
            enabled: true,
            certificate_id: Some(id),
            ..HttpsPolicy::default()
        };
        self.service.set_subsystem_https(site, https).await.unwrap();
        id
    }

    pub fn hello(&self, sni: &str, ca: &TestCa) -> ClientHello {
        ClientHello {
            sni: Some(sni.to_string()),
            trust_pem: vec![ca.pem.clone()],
            ..ClientHello::default()
        }
    }

    pub async fn leaf_seen(&self, sni: &str) -> Vec<u8> {
        let hello = ClientHello {
            sni: Some(sni.to_string()),
            insecure: true,
            ..ClientHello::default()
        };
        handshake(self.https, &hello).await.unwrap().leaf_der
    }

    /// Waits until the metrics text contains `needle`.
    pub async fn metric(&self, needle: &str) {
        for _ in 0..200 {
            if self.metrics.render().contains(needle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("metric never appeared: {needle}\n{}", self.metrics.render());
    }
}

pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
