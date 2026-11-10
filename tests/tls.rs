//! TLS termination end to end (M3): the real data plane and config service in
//! one process with real handshakes, plus the real binary for the switch.
//! Certificates are issued by an in-memory CA and uploaded as PEM strings, so
//! no files are involved.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_config_service::{CertificateChange, ConfigService, TlsSettings};
use fast_proxy_dataplane::{
    bind, DataPlane, ListenerSpec, Observability, RequestPipeline, TlsPlane, TlsPlaneSettings,
    TrustedProxies, UpstreamClient,
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
use fast_proxy_test_pki::{handshake, request, ClientHello, LeafSpec, TestCa};
use fast_proxy_tls::{MinTlsVersion, ServerTlsPolicy};
use rustls::ProtocolVersion;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

struct Options {
    min_version: MinTlsVersion,
    reject_unknown_sni: bool,
    handshake_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            min_version: MinTlsVersion::V1_2,
            reject_unknown_sni: false,
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

struct Stack {
    service: Arc<ConfigService>,
    metrics: Arc<Metrics>,
    https: SocketAddr,
    http: SocketAddr,
    backend: SocketAddr,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// A backend answering every request with the request head it received.
async fn echo_backend() -> SocketAddr {
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
    async fn start(options: Options) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let metrics = Arc::new(Metrics::new());
        let store = Store::open(&dir.path().join("fast-proxy.db")).unwrap();
        let settings = TlsSettings {
            enabled: true,
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
        let pipeline = Arc::new(RequestPipeline::new(
            service.snapshot_holder(),
            forwarder,
            observability,
        ));
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
        let tls = TlsPlane::new(
            https,
            service.snapshot_holder(),
            TlsPlaneSettings {
                policy: ServerTlsPolicy {
                    min_version: options.min_version,
                    http2: false,
                },
                handshake_timeout: options.handshake_timeout,
                max_concurrent_handshakes: 64,
                reject_unknown_sni: options.reject_unknown_sni,
            },
            metrics.clone(),
        )
        .unwrap();
        let shutdown = CancellationToken::new();
        let plane = DataPlane::start_with_tls(http, Some(tls), pipeline, shutdown.clone());
        let addr_of = |name: &str| {
            plane
                .local_addrs()
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, addr)| *addr)
                .unwrap()
        };
        let (http, https) = (addr_of("http"), addr_of("https"));
        tokio::spawn(async move {
            let _ = plane.stopped(Duration::from_secs(1)).await;
        });
        Self {
            service,
            metrics,
            https,
            http,
            backend: echo_backend().await,
            shutdown,
            _dir: dir,
        }
    }

    /// A subsystem answering to `host`, routing `/` to the echo backend.
    async fn add_site(&self, name: &'static str, host: &'static str) -> SubsystemId {
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
    async fn serve_with(
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

    fn hello(&self, sni: &str, ca: &TestCa) -> ClientHello {
        ClientHello {
            sni: Some(sni.to_string()),
            trust_pem: vec![ca.pem.clone()],
            ..ClientHello::default()
        }
    }

    async fn leaf_seen(&self, sni: &str) -> Vec<u8> {
        let hello = ClientHello {
            sni: Some(sni.to_string()),
            insecure: true,
            ..ClientHello::default()
        };
        handshake(self.https, &hello).await.unwrap().leaf_der
    }

    /// Waits until the metrics text contains `needle`.
    async fn metric(&self, needle: &str) {
        for _ in 0..200 {
            if self.metrics.render().contains(needle) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("metric never appeared: {needle}\n{}", self.metrics.render());
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn each_subsystem_is_served_with_its_own_certificate() {
    let stack = Stack::start(Options::default()).await;
    let (shop_ca, blog_ca) = (TestCa::new("shop ca"), TestCa::new("blog ca"));
    let shop = stack.add_site("shop", "shop.example.com").await;
    let blog = stack.add_site("blog", "blog.example.com").await;
    stack
        .serve_with(shop, "shop", &shop_ca, &["shop.example.com"])
        .await;
    stack
        .serve_with(blog, "blog", &blog_ca, &["blog.example.com"])
        .await;

    handshake(stack.https, &stack.hello("shop.example.com", &shop_ca))
        .await
        .unwrap();
    handshake(stack.https, &stack.hello("blog.example.com", &blog_ca))
        .await
        .unwrap();
    assert!(
        handshake(stack.https, &stack.hello("shop.example.com", &blog_ca))
            .await
            .is_err()
    );
    assert!(
        handshake(stack.https, &stack.hello("blog.example.com", &shop_ca))
            .await
            .is_err()
    );
    stack.metric("fastproxy_tls_handshakes_total{listener=\"https\",result=\"success\",version=\"TLSv1_3\"}").await;
}

#[tokio::test]
async fn two_subsystems_can_share_one_wildcard_certificate() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    let blog = stack.add_site("blog", "blog.example.com").await;
    let id = stack
        .serve_with(shop, "wild", &ca, &["*.example.com"])
        .await;
    let https = HttpsPolicy {
        enabled: true,
        certificate_id: Some(id),
        ..HttpsPolicy::default()
    };
    stack
        .service
        .set_subsystem_https(blog, https)
        .await
        .unwrap();

    let shop_leaf = stack.leaf_seen("shop.example.com").await;
    assert_eq!(shop_leaf, stack.leaf_seen("blog.example.com").await);
    handshake(stack.https, &stack.hello("blog.example.com", &ca))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_unknown_name_or_no_sni_gets_the_placeholder_and_a_404() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;

    assert!(contains(
        &stack.leaf_seen("nobody.example.com").await,
        b"fast-proxy.invalid"
    ));
    assert!(
        handshake(stack.https, &stack.hello("nobody.example.com", &ca))
            .await
            .is_err()
    );
    let no_sni = ClientHello {
        sni: None,
        insecure: true,
        ..ClientHello::default()
    };
    let negotiated = handshake(stack.https, &no_sni).await.unwrap();
    assert!(contains(&negotiated.leaf_der, b"fast-proxy.invalid"));

    let hello = ClientHello {
        sni: Some("nobody.example.com".into()),
        insecure: true,
        ..ClientHello::default()
    };
    let answer = request(
        stack.https,
        &hello,
        b"GET / HTTP/1.1\r\nHost: nobody.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(answer.starts_with("HTTP/1.1 404"), "{answer}");
}

#[tokio::test]
async fn a_subsystem_that_did_not_opt_in_to_https_gets_the_placeholder() {
    let stack = Stack::start(Options::default()).await;
    stack.add_site("plain", "plain.example.com").await;
    assert!(contains(
        &stack.leaf_seen("plain.example.com").await,
        b"fast-proxy.invalid"
    ));
}

#[tokio::test]
async fn reject_unknown_sni_fails_the_handshake_and_counts_it() {
    let stack = Stack::start(Options {
        reject_unknown_sni: true,
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;
    let insecure = |sni: Option<&str>| ClientHello {
        sni: sni.map(str::to_string),
        insecure: true,
        ..ClientHello::default()
    };
    assert!(
        handshake(stack.https, &insecure(Some("nobody.example.com")))
            .await
            .is_err()
    );
    assert!(handshake(stack.https, &insecure(None)).await.is_err());
    handshake(stack.https, &stack.hello("shop.example.com", &ca))
        .await
        .unwrap();
    stack.metric("result=\"rejected_sni\"").await;
}

#[tokio::test]
async fn failed_handshakes_are_counted_and_do_not_stop_the_listener() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;
    let mut garbage = TcpStream::connect(stack.https).await.unwrap();
    garbage
        .write_all(b"GET / HTTP/1.1\r\nHost: shop.example.com\r\n\r\n")
        .await
        .unwrap();
    let mut sink = Vec::new();
    let _ = garbage.read_to_end(&mut sink).await;
    stack.metric("result=\"failed\"").await;
    handshake(stack.https, &stack.hello("shop.example.com", &ca))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_stalled_handshake_is_cut_off_at_the_timeout() {
    let stack = Stack::start(Options {
        handshake_timeout: Duration::from_millis(300),
        ..Options::default()
    })
    .await;
    let mut stalled = TcpStream::connect(stack.https).await.unwrap();
    let mut sink = Vec::new();
    let closed =
        tokio::time::timeout(Duration::from_secs(20), stalled.read_to_end(&mut sink)).await;
    assert!(
        closed.is_ok(),
        "the server must close a connection that never handshakes"
    );
    stack.metric("result=\"timeout\"").await;
}

#[tokio::test]
async fn a_replaced_certificate_reaches_the_next_handshake() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    let id = stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;
    let before = stack.leaf_seen("shop.example.com").await;

    let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    stack
        .service
        .replace_certificate(
            id,
            CertificateChange {
                cert_pem: Some(renewed.chain_pem),
                key_pem: Some(renewed.key_pem),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    let after = stack.leaf_seen("shop.example.com").await;
    assert_ne!(
        before, after,
        "no restart, the next handshake serves the new certificate"
    );
    handshake(stack.https, &stack.hello("shop.example.com", &ca))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_dev_certificate_is_served_the_moment_https_is_enabled() {
    let stack = Stack::start(Options::default()).await;
    let site = stack.add_site("shop", "shop.example.com").await;
    let https = HttpsPolicy {
        enabled: true,
        ..HttpsPolicy::default()
    };
    stack
        .service
        .set_subsystem_https(site, https)
        .await
        .unwrap();
    let leaf = stack.leaf_seen("shop.example.com").await;
    assert!(contains(&leaf, b"shop.example.com"));
    assert!(!contains(&leaf, b"fast-proxy.invalid"));
}

#[tokio::test]
async fn requests_over_https_are_proxied_and_marked_https_to_the_backend() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;
    let raw = b"GET /cart HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n";

    let secure = request(stack.https, &stack.hello("shop.example.com", &ca), raw)
        .await
        .unwrap();
    assert!(secure.starts_with("HTTP/1.1 200"), "{secure}");
    assert!(secure.contains("get /cart http/1.1"), "{secure}");
    assert!(secure.contains("x-forwarded-proto: https"), "{secure}");

    let mut plain = TcpStream::connect(stack.http).await.unwrap();
    plain.write_all(raw).await.unwrap();
    let mut answer = String::new();
    plain.read_to_string(&mut answer).await.unwrap();
    assert!(answer.contains("x-forwarded-proto: http\r\n"), "{answer}");
}

#[tokio::test]
async fn the_minimum_version_is_enforced() {
    let stack = Stack::start(Options {
        min_version: MinTlsVersion::V1_3,
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(shop, "shop", &ca, &["shop.example.com"])
        .await;
    let mut legacy = stack.hello("shop.example.com", &ca);
    legacy.only_version = Some(ProtocolVersion::TLSv1_2);
    assert!(handshake(stack.https, &legacy).await.is_err());
    let modern = handshake(stack.https, &stack.hello("shop.example.com", &ca))
        .await
        .unwrap();
    assert_eq!(modern.version, Some(ProtocolVersion::TLSv1_3));
}

// ---- the process-wide switch, through the real binary -----------------------

use common::{free_port, try_start_with_args};

fn tls_section(listen_port: u16, enabled: bool) -> String {
    format!("\n[data_plane.tls]\nenabled = {enabled}\nlisten = \"127.0.0.1:{listen_port}\"\n")
}

/// Starts the binary, retrying on a port that a parallel test took.
fn start_binary(
    dir: &std::path::Path,
    enabled_in_file: bool,
    args: &[&str],
) -> (common::Proxy, u16) {
    for _ in 0..5 {
        let https_port = free_port();
        let tail = tls_section(https_port, enabled_in_file);
        let mut full_args = args.to_vec();
        let flag = format!("127.0.0.1:{https_port}");
        full_args.extend(["--https-listen", flag.as_str()]);
        if let Ok(proxy) =
            try_start_with_args(dir, free_port(), free_port(), ("", &tail), &full_args)
        {
            return (proxy, https_port);
        }
    }
    panic!("the proxy would not start on any of 5 port sets");
}

fn https_address(port: u16) -> SocketAddr {
    ([127, 0, 0, 1], port).into()
}

fn insecure() -> ClientHello {
    ClientHello {
        sni: Some("anything.example.com".into()),
        insecure: true,
        ..ClientHello::default()
    }
}

#[tokio::test]
async fn tls_is_off_by_default_so_there_is_no_https_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (_proxy, port) = start_binary(dir.path(), false, &[]);
    assert!(handshake(https_address(port), &insecure()).await.is_err());
    assert!(!dir.path().join("state").join("master.key").exists());
}

#[tokio::test]
async fn the_config_file_switch_starts_the_https_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (_proxy, port) = start_binary(dir.path(), true, &[]);
    let negotiated = handshake(https_address(port), &insecure()).await.unwrap();
    assert!(
        contains(&negotiated.leaf_der, b"fast-proxy.invalid"),
        "the placeholder answers unknown names"
    );
}

#[tokio::test]
async fn the_tls_flag_overrides_the_file_and_no_tls_turns_it_off_again() {
    let dir = tempfile::tempdir().unwrap();
    let (_proxy, port) = start_binary(dir.path(), false, &["--tls"]);
    assert!(handshake(https_address(port), &insecure()).await.is_ok());

    let dir = tempfile::tempdir().unwrap();
    let (_proxy, port) = start_binary(dir.path(), true, &["--no-tls"]);
    assert!(handshake(https_address(port), &insecure()).await.is_err());
}

// ---- driven through the admin API of the real binary ---------------------------

fn trusting(sni: &str, ca: &TestCa) -> ClientHello {
    ClientHello {
        sni: Some(sni.to_string()),
        trust_pem: vec![ca.pem.clone()],
        ..ClientHello::default()
    }
}

#[tokio::test]
async fn a_certificate_uploaded_through_the_api_is_served_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (proxy, https_port) = start_binary(dir.path(), true, &[]);
    let (data_port, admin_port) = (proxy.data.port(), proxy.admin.port());
    let backend = common::start_backend("shop-backend");
    let (site, _) = common::create_site(&proxy, "shop", "shop.example.com", backend);

    let ca = TestCa::new("shop ca");
    let issued = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, body) = common::admin(
        &proxy,
        "POST",
        &format!("/api/subsystems/{site}/certificate"),
        Some(
            serde_json::json!({ "name": "shop", "certPem": issued.chain_pem, "keyPem": issued.key_pem }),
        ),
    );
    assert_eq!(status, 201, "{body}");
    assert!(!body.to_string().contains("PRIVATE"));
    let (status, body) = common::admin(
        &proxy,
        "PUT",
        &format!("/api/subsystems/{site}"),
        Some(serde_json::json!({ "name": "shop", "https": {
            "enabled": true, "certificateId": body["data"]["id"] } })),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["data"]["https"]["effective"], true);

    let address = https_address(https_port);
    handshake(address, &trusting("shop.example.com", &ca))
        .await
        .unwrap();
    let answer = request(
        address,
        &trusting("shop.example.com", &ca),
        b"GET /cart HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(answer.contains("shop-backend /cart"), "{answer}");
    let (_, tls) = common::admin(&proxy, "GET", "/api/tls", None);
    assert_eq!(tls["data"]["vault"], "ok");

    drop(proxy);
    let tail = tls_section(https_port, true);
    let restarted = try_start_with_args(dir.path(), data_port, admin_port, ("", &tail), &[])
        .expect("restarts on the same ports");
    handshake(address, &trusting("shop.example.com", &ca))
        .await
        .unwrap();
    drop(restarted);
}
