//! The M3 exit criteria, end to end through the real binary: TLS off by default
//! and switchable, a development certificate the moment HTTPS is enabled, real
//! certificates uploaded and replaced live, SNI per subsystem, HTTP/1.1 and
//! HTTP/2 on one port, the 421 rule, redirect and HSTS live (and inert with TLS
//! off), the minimum version, a TLS upstream with an uploaded CA, TLS health
//! checks, and everything surviving a restart. Certificates are PEM strings made
//! by an in-memory CA and sent through the admin API; no files are involved.

mod common;

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use common::tls_stack::{contains, tls_backend};
use common::{admin, create_site, free_port, start_backend, try_start_with_args, Proxy};
use fast_proxy_test_pki::{connect, handshake, request, ClientHello, Issued, LeafSpec, TestCa};
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::ProtocolVersion;
use serde_json::{json, Value};

struct Running {
    proxy: Proxy,
    https: SocketAddr,
    dir: tempfile::TempDir,
    ports: (u16, u16, u16),
}

fn tls_tail(https_port: u16, extra: &str) -> String {
    format!("\n[data_plane.tls]\nenabled = true\nlisten = \"127.0.0.1:{https_port}\"\n{extra}")
}

/// Starts the binary with TLS on (a shared master key from the environment is
/// not needed: the key file lands in the state directory), retrying on ports a
/// parallel test took.
fn start(tls_extra: &str) -> Running {
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let (data, admin_port, https) = (free_port(), free_port(), free_port());
        if let Ok(proxy) = try_start_with_args(
            dir.path(),
            data,
            admin_port,
            ("", &tls_tail(https, tls_extra)),
            &[],
        ) {
            return Running {
                proxy,
                https: ([127, 0, 0, 1], https).into(),
                dir,
                ports: (data, admin_port, https),
            };
        }
    }
    panic!("the proxy would not start on any of 5 port sets");
}

fn restart(running: Running, tls_extra: &str) -> Running {
    let (data, admin_port, https) = running.ports;
    let dir = running.dir;
    drop(running.proxy);
    let proxy = try_start_with_args(
        dir.path(),
        data,
        admin_port,
        ("", &tls_tail(https, tls_extra)),
        &[],
    )
    .expect("restarts on the same ports");
    Running {
        proxy,
        https: ([127, 0, 0, 1], https).into(),
        dir,
        ports: (data, admin_port, https),
    }
}

fn hello(sni: &str, ca: Option<&TestCa>) -> ClientHello {
    ClientHello {
        sni: Some(sni.into()),
        trust_pem: ca.map(|ca| vec![ca.pem.clone()]).unwrap_or_default(),
        insecure: ca.is_none(),
        ..ClientHello::default()
    }
}

async fn leaf(running: &Running, sni: &str) -> Vec<u8> {
    handshake(running.https, &hello(sni, None))
        .await
        .unwrap()
        .leaf_der
}

fn upload_body(name: &str, issued: &Issued) -> Value {
    json!({ "name": name, "certPem": issued.chain_pem, "keyPem": issued.key_pem })
}

fn enable_https(running: &Running, site: i64, name: &str, extra: Value) -> Value {
    let mut https = json!({ "enabled": true });
    if let (Some(target), Some(extra)) = (https.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    let (status, body) = admin(
        &running.proxy,
        "PUT",
        &format!("/api/subsystems/{site}"),
        Some(json!({ "name": name, "https": https })),
    );
    assert_eq!(status, 200, "{body}");
    body
}

fn config_version(running: &Running) -> u64 {
    admin(&running.proxy, "GET", "/api/subsystems", None).1["version"]
        .as_u64()
        .unwrap()
}

type H2 = hyper::client::conn::http2::SendRequest<Empty<Bytes>>;

async fn h2_connection(running: &Running, sni: &str, ca: Option<&TestCa>) -> H2 {
    let mut client = hello(sni, ca);
    client.alpn = vec!["h2".into()];
    let (tls, negotiated) = connect(running.https, &client).await.unwrap();
    assert_eq!(negotiated.alpn.as_deref(), Some(&b"h2"[..]));
    let (send, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(connection);
    send
}

async fn h2_get(client: &mut H2, host: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .uri(format!("https://{host}/page"))
        .body(Empty::<Bytes>::new())
        .unwrap();
    let response = client.send_request(request).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn eventually(what: &str, mut check: impl AsyncFnMut() -> bool) {
    for _ in 0..300 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("never happened: {what}");
}

#[tokio::test]
async fn a_dev_certificate_then_real_ones_per_subsystem_replaced_and_deleted_live() {
    let running = start("");
    let (shop, blog) = (start_backend("shop-backend"), start_backend("blog-backend"));
    let (shop_id, _) = create_site(&running.proxy, "shop", "shop.example.com", shop);
    let (blog_id, _) = create_site(&running.proxy, "blog", "blog.example.com", blog);

    // Enabling HTTPS gives each subsystem a certificate the next handshake serves.
    enable_https(&running, shop_id, "shop", json!({}));
    enable_https(&running, blog_id, "blog", json!({}));
    let dev_shop = leaf(&running, "shop.example.com").await;
    assert!(contains(&dev_shop, b"shop.example.com"));
    assert!(!contains(&dev_shop, b"fast-proxy.invalid"));

    // A real certificate for the shop only: SNI picks it, the blog keeps its own.
    let ca = TestCa::new("shop ca");
    let issued = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, uploaded) = admin(
        &running.proxy,
        "POST",
        &format!("/api/subsystems/{shop_id}/certificate"),
        Some(upload_body("shop", &issued)),
    );
    assert_eq!(status, 201, "{uploaded}");
    let cert_id = uploaded["data"]["id"].as_i64().unwrap();
    handshake(running.https, &hello("shop.example.com", Some(&ca)))
        .await
        .unwrap();
    assert!(
        handshake(running.https, &hello("blog.example.com", Some(&ca)))
            .await
            .is_err()
    );

    // HTTP/1.1 and HTTP/2 on the one port, proxied.
    let mut h2 = h2_connection(&running, "shop.example.com", Some(&ca)).await;
    let (status, body) = h2_get(&mut h2, "shop.example.com").await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::OK, "shop-backend /page")
    );
    let mut h1 = hello("shop.example.com", Some(&ca));
    h1.alpn = vec!["http/1.1".into()];
    let answer = request(
        running.https,
        &h1,
        b"GET /page HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(answer.contains("shop-backend /page"), "{answer}");

    // An invalid upload changes nothing: same version, same certificate served.
    let before = (
        config_version(&running),
        leaf(&running, "shop.example.com").await,
    );
    for body in [
        json!({ "name": "bad", "certPem": "garbage", "keyPem": issued.key_pem }),
        json!({ "name": "bad", "certPem": issued.cert_pem, "keyPem": "garbage" }),
        json!({ "name": "bad", "certPem": issued.cert_pem, "keyPem": ca.issue(&LeafSpec::new(&["x.example.com"])).key_pem }),
    ] {
        let (status, rejected) = admin(&running.proxy, "POST", "/api/certificates", Some(body));
        assert_eq!(status, 400, "{rejected}");
        assert!(
            !rejected.to_string().contains("garbage"),
            "errors never echo PEM"
        );
    }
    assert_eq!(
        before,
        (
            config_version(&running),
            leaf(&running, "shop.example.com").await
        )
    );

    // Replacing the certificate takes effect with no restart.
    let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, body) = admin(
        &running.proxy,
        "PUT",
        &format!("/api/certificates/{cert_id}"),
        Some(json!({ "certPem": renewed.chain_pem, "keyPem": renewed.key_pem })),
    );
    assert_eq!(status, 200, "{body}");
    assert_ne!(leaf(&running, "shop.example.com").await, before.1);
    handshake(running.https, &hello("shop.example.com", Some(&ca)))
        .await
        .unwrap();

    // It survives a restart.
    let running = restart(running, "");
    handshake(running.https, &hello("shop.example.com", Some(&ca)))
        .await
        .unwrap();

    // Detaching falls back to a development certificate; the real one can then be deleted.
    let (status, _) = admin(
        &running.proxy,
        "DELETE",
        &format!("/api/subsystems/{shop_id}/certificate"),
        None,
    );
    assert_eq!(status, 200);
    let (status, _) = admin(
        &running.proxy,
        "DELETE",
        &format!("/api/certificates/{cert_id}"),
        None,
    );
    assert_eq!(status, 200);
    assert!(
        handshake(running.https, &hello("shop.example.com", Some(&ca)))
            .await
            .is_err()
    );
    assert!(contains(
        &leaf(&running, "shop.example.com").await,
        b"shop.example.com"
    ));
}

#[tokio::test]
async fn the_421_rule_redirect_and_hsts_work_live_and_are_inert_with_tls_off() {
    let running = start("");
    let (shop, blog) = (start_backend("shop-backend"), start_backend("blog-backend"));
    let (shop_id, _) = create_site(&running.proxy, "shop", "shop.example.com", shop);
    let (blog_id, _) = create_site(&running.proxy, "blog", "blog.example.com", blog);
    let (ca_shop, ca_blog) = (TestCa::new("shop ca"), TestCa::new("blog ca"));
    for (id, name, host, ca) in [
        (shop_id, "shop", "shop.example.com", &ca_shop),
        (blog_id, "blog", "blog.example.com", &ca_blog),
    ] {
        let issued = ca.issue(&LeafSpec::new(&[host]));
        let (status, uploaded) = admin(
            &running.proxy,
            "POST",
            &format!("/api/subsystems/{id}/certificate"),
            Some(upload_body(name, &issued)),
        );
        assert_eq!(status, 201, "{uploaded}");
        enable_https(
            &running,
            id,
            name,
            json!({ "certificateId": uploaded["data"]["id"] }),
        );
    }

    // A host served by another certificate than the connection's gets 421.
    let mut client = h2_connection(&running, "shop.example.com", Some(&ca_shop)).await;
    assert_eq!(
        h2_get(&mut client, "shop.example.com").await.0,
        StatusCode::OK
    );
    assert_eq!(
        h2_get(&mut client, "blog.example.com").await.0,
        StatusCode::MISDIRECTED_REQUEST
    );

    // Redirect and HSTS, live.
    let plain =
        |path: &str| common::http(running.proxy.data, "GET", path, "shop.example.com", None);
    assert_eq!(plain("/cart?x=1").0, 200, "no redirect yet");
    enable_https(
        &running,
        shop_id,
        "shop",
        json!({
            "redirect": "308",
            "hsts": { "enabled": true, "maxAgeSecs": 600 },
            "certificateId": admin(&running.proxy, "GET", &format!("/api/subsystems/{shop_id}"), None).1["data"]["https"]["certificateId"],
        }),
    );
    let mut raw = String::new();
    {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(running.proxy.data).unwrap();
        stream
            .write_all(
                b"GET /cart?x=1 HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.read_to_string(&mut raw).unwrap();
    }
    assert!(raw.starts_with("HTTP/1.1 308"), "{raw}");
    assert!(
        raw.contains(&format!(
            "location: https://shop.example.com:{}/cart?x=1",
            running.ports.2
        )),
        "{raw}"
    );
    let secure = request(
        running.https,
        &hello("shop.example.com", Some(&ca_shop)),
        b"GET / HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(
        secure
            .to_lowercase()
            .contains("strict-transport-security: max-age=600"),
        "{secure}"
    );

    // The same configuration is inert after a restart with TLS off.
    let (data, admin_port) = (running.ports.0, running.ports.1);
    let dir = running.dir;
    drop(running.proxy);
    let off = try_start_with_args(dir.path(), data, admin_port, ("", ""), &["--no-tls"]).unwrap();
    let (status, body) = common::http(off.data, "GET", "/cart?x=1", "shop.example.com", None);
    assert_eq!(
        (status, body.as_str()),
        (200, "shop-backend /cart?x=1"),
        "no redirect to a listener that is not there"
    );
    let (_, detail) = admin(&off, "GET", &format!("/api/subsystems/{shop_id}"), None);
    assert_eq!(
        detail["data"]["https"]["redirect"], "308",
        "stored, not lost"
    );
    assert_eq!(detail["data"]["https"]["effective"], false);
}

#[tokio::test]
async fn the_minimum_tls_version_comes_from_the_config_file() {
    let running = start("min_version = \"1.3\"\n");
    let backend = start_backend("shop-backend");
    let (id, _) = create_site(&running.proxy, "shop", "shop.example.com", backend);
    enable_https(&running, id, "shop", json!({}));
    let mut legacy = hello("shop.example.com", None);
    legacy.only_version = Some(ProtocolVersion::TLSv1_2);
    assert!(handshake(running.https, &legacy).await.is_err());
    let modern = handshake(running.https, &hello("shop.example.com", None))
        .await
        .unwrap();
    assert_eq!(modern.version, Some(ProtocolVersion::TLSv1_3));
}

// Multi-threaded: the blocking HTTP helper must not stop the in-process TLS backend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tls_upstream_is_verified_against_an_uploaded_ca_and_its_health_is_checked_over_tls() {
    let running = start("");
    let trusted = TestCa::new("backend ca");
    let stranger = TestCa::new("someone else");
    let (backend, served) = tls_backend(&trusted, &["backend.internal"]).await;
    let port = backend.port();

    for (name, ca) in [("good", &trusted), ("other", &stranger)] {
        let (status, body) = admin(
            &running.proxy,
            "POST",
            "/api/ca-bundles",
            Some(json!({ "name": name, "pem": ca.pem })),
        );
        assert_eq!(status, 201, "{body}");
    }
    let make = |name: &str, host: &str, bundle: &str| {
        let (status, site) = admin(
            &running.proxy,
            "POST",
            "/api/subsystems",
            Some(json!({ "name": name })),
        );
        assert_eq!(status, 201, "{site}");
        let id = site["data"]["id"].as_i64().unwrap();
        let base = format!("/api/subsystems/{id}");
        admin(
            &running.proxy,
            "POST",
            &format!("{base}/hosts"),
            Some(json!({ "pattern": host })),
        );
        let (status, upstream) = admin(
            &running.proxy,
            "POST",
            &format!("{base}/upstreams"),
            Some(json!({
                "name": "backend",
                "tls": { "enabled": true, "serverName": "backend.internal", "ca": bundle },
                "health": { "check": "http", "path": "/", "intervalMs": 200, "timeoutMs": 150, "rise": 1, "fall": 1 },
                "endpoints": [{ "addr": format!("127.0.0.1:{port}") }]
            })),
        );
        assert_eq!(status, 201, "{upstream}");
        let group = upstream["data"]["id"].as_i64().unwrap();
        let (status, _) = admin(
            &running.proxy,
            "POST",
            &format!("{base}/routes"),
            Some(json!({ "prefix": "/", "upstreamGroupId": group })),
        );
        assert_eq!(status, 201);
        (id, group)
    };
    let (good_id, good_group) = make("good", "good.example.com", "good");
    let (bad_id, bad_group) = make("bad", "bad.example.com", "other");

    let (status, body) = common::http(running.proxy.data, "GET", "/", "good.example.com", None);
    assert_eq!((status, body.as_str()), (200, "tls-ok"));
    let before = served.load(std::sync::atomic::Ordering::SeqCst);
    let (status, _) = common::http(running.proxy.data, "GET", "/", "bad.example.com", None);
    assert_eq!(
        status, 502,
        "a certificate the proxy does not trust is refused"
    );
    assert!(before >= 1);

    // The health checks use TLS with each group's own settings.
    let state = |id: i64, group: i64| {
        let (_, status) = admin(
            &running.proxy,
            "GET",
            &format!("/api/subsystems/{id}/upstreams/{group}/status"),
            None,
        );
        status["data"]["endpoints"][0]["effective"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    eventually("the trusted TLS endpoint is healthy", async || {
        state(good_id, good_group) == "healthy"
    })
    .await;
    eventually("the untrusted one is unhealthy", async || {
        matches!(state(bad_id, bad_group).as_str(), "unhealthy" | "ejected")
    })
    .await;
}
