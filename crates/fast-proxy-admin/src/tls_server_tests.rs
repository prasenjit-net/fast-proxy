//! The admin plane served over TLS (ADR 0016): HTTPS with the stored `admin`
//! certificate, the peer address, the WebSocket, a replacement served live,
//! graceful shutdown, and the rules that keep that certificate in place.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use fast_proxy_config_service::ChangeContext;
use fast_proxy_store::repository::audit::{self, AuditQuery};
use fast_proxy_test_pki::{handshake, request, ClientHello, LeafSpec, TestCa};
use fast_proxy_tls::{build_server_config, ServerTlsPolicy};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::test_support::TestApp;
use crate::{serve_tls, StoredCertificate};

/// An admin plane served over TLS on a loopback port, with the generated
/// `admin` certificate.
struct TlsAdmin {
    app: TestApp,
    addr: SocketAddr,
    stop: CancellationToken,
    server: tokio::task::JoinHandle<()>,
}

impl TlsAdmin {
    async fn start() -> Self {
        let app = TestApp::with_admin_tls().await;
        app.config
            .ensure_self_signed_certificate(
                &ChangeContext::system(),
                "admin",
                vec!["localhost".into()],
            )
            .await
            .unwrap();
        let resolver = Arc::new(StoredCertificate::new(app.config.clone(), "admin"));
        let tls = build_server_config(&ServerTlsPolicy::default(), resolver).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let server = tokio::spawn(serve_tls(
            listener,
            app.router(),
            Arc::new(tls),
            stop.clone(),
        ));
        Self {
            app,
            addr,
            stop,
            server,
        }
    }

    fn hello() -> ClientHello {
        ClientHello {
            sni: Some("localhost".into()),
            insecure: true,
            alpn: vec!["http/1.1".into()],
            ..ClientHello::default()
        }
    }

    /// One request over TLS; returns the whole answer.
    async fn send(&self, method: &str, path: &str, extra: &str, body: &str) -> String {
        let token = self.app.token.clone().unwrap();
        let raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost:9000\r\nAuthorization: Bearer {token}\r\n\
             {extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        request(self.addr, &Self::hello(), raw.as_bytes())
            .await
            .expect("the TLS request completes")
    }
}

fn status_line(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

#[tokio::test]
async fn the_api_is_served_over_https_with_the_generated_certificate() {
    let admin = TlsAdmin::start().await;
    let answer = admin.send("GET", "/api/whoami", "", "").await;
    assert!(status_line(&answer).contains("200"), "{answer}");
    assert!(answer.contains("test-admin"), "{answer}");
    assert!(answer.to_lowercase().contains("content-security-policy"));

    let negotiated = handshake(admin.addr, &TlsAdmin::hello()).await.unwrap();
    let generated = admin.app.config.certificate_by_name("admin").unwrap();
    assert_eq!(
        negotiated.leaf_der,
        generated.key.cert[0].as_ref(),
        "the stored certificate is the one presented"
    );
}

#[tokio::test]
async fn requests_over_tls_carry_their_peer_address() {
    let admin = TlsAdmin::start().await;
    let answer = admin
        .send(
            "POST",
            "/api/subsystems",
            "",
            &json!({ "name": "shop" }).to_string(),
        )
        .await;
    assert!(status_line(&answer).contains("201"), "{answer}");
    let entries = admin
        .app
        .config
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 10))
        .await
        .unwrap()
        .value;
    assert_eq!(entries[0].entry.source, "127.0.0.1");
    assert_eq!(entries[0].entry.actor, "test-admin");
}

#[tokio::test]
async fn the_websocket_upgrades_through_tls_with_a_ticket() {
    let admin = TlsAdmin::start().await;
    let answer = admin.send("POST", "/api/ws-ticket", "", "").await;
    let ticket = answer
        .split("\"ticket\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("a ticket")
        .to_string();

    let (mut tls, _) = fast_proxy_test_pki::connect(admin.addr, &TlsAdmin::hello())
        .await
        .unwrap();
    let upgrade = format!(
        "GET /ws?ticket={ticket} HTTP/1.1\r\nHost: localhost:9000\r\nConnection: Upgrade\r\n\
         Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
    );
    tls.write_all(upgrade.as_bytes()).await.unwrap();
    let mut buffer = [0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(5), tls.read(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&buffer[..n]).contains("101"),
        "{}",
        String::from_utf8_lossy(&buffer[..n])
    );
}

#[tokio::test]
async fn an_uploaded_certificate_replaces_the_generated_one_for_the_next_connection() {
    let admin = TlsAdmin::start().await;
    let before = handshake(admin.addr, &TlsAdmin::hello()).await.unwrap();

    let ca = TestCa::new("ca");
    let issued = ca.issue(&LeafSpec::new(&["localhost"]));
    let current = admin.app.config.certificate_by_name("admin").unwrap();
    admin
        .app
        .config
        .replace_certificate(
            &ChangeContext::system(),
            current.id,
            fast_proxy_config_service::CertificateChange {
                cert_pem: Some(issued.chain_pem.clone()),
                key_pem: Some(issued.key_pem.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let trusted = ClientHello {
        sni: Some("localhost".into()),
        trust_pem: vec![issued.ca_pem.clone()],
        alpn: vec!["http/1.1".into()],
        ..ClientHello::default()
    };
    let after = handshake(admin.addr, &trusted)
        .await
        .expect("the CA-issued certificate verifies");
    assert_ne!(before.leaf_der, after.leaf_der);
}

#[tokio::test]
async fn the_plain_port_is_not_open_when_tls_is_on() {
    let admin = TlsAdmin::start().await;
    let mut tcp = tokio::net::TcpStream::connect(admin.addr).await.unwrap();
    tcp.write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut answer)).await;
    assert!(
        !String::from_utf8_lossy(&answer).starts_with("HTTP/"),
        "a plain request must not be answered over HTTP"
    );
}

#[tokio::test]
async fn a_bad_host_is_refused_over_tls_too() {
    let admin = TlsAdmin::start().await;
    let raw = "GET /api/health HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n";
    let answer = request(admin.addr, &TlsAdmin::hello(), raw.as_bytes())
        .await
        .unwrap();
    assert!(status_line(&answer).contains("421"), "{answer}");
}

#[tokio::test]
async fn shutdown_stops_the_listener_and_lets_the_server_finish() {
    let admin = TlsAdmin::start().await;
    assert!(status_line(&admin.send("GET", "/api/health", "", "").await).contains("200"));
    admin.stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), admin.server)
        .await
        .expect("the server stops")
        .unwrap();
    assert!(handshake(admin.addr, &TlsAdmin::hello()).await.is_err());
}

// ---- keeping the served certificate in place ----

async fn admin_certificate_id(app: &TestApp) -> i64 {
    app.config
        .ensure_self_signed_certificate(&ChangeContext::system(), "admin", vec!["localhost".into()])
        .await
        .unwrap();
    app.config.certificate_by_name("admin").unwrap().id.0
}

#[tokio::test]
async fn the_certificate_the_admin_plane_serves_cannot_be_deleted_or_renamed() {
    let app = TestApp::with_admin_tls().await;
    let id = admin_certificate_id(&app).await;
    let uri = format!("/api/certificates/{id}");

    let (status, body) = app.call(Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("\"admin\""));
    let (status, _) = app
        .call(Method::PUT, &uri, Some(json!({ "name": "renamed" })))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(app.config.certificate_by_name("admin").is_some());
}

#[tokio::test]
async fn it_can_be_renewed_and_other_certificates_are_unrestricted() {
    let app = TestApp::with_admin_tls().await;
    let id = admin_certificate_id(&app).await;
    let ca = TestCa::new("ca");
    let issued = ca.issue(&LeafSpec::new(&["localhost"]));

    let (status, body) = app
        .call(
            Method::PUT,
            &format!("/api/certificates/{id}"),
            Some(json!({ "certPem": issued.chain_pem, "keyPem": issued.key_pem, "name": "admin" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let other = app
        .config
        .generate_self_signed(
            &ChangeContext::system(),
            "other",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap()
        .value;
    let (status, _) = app
        .call(
            Method::DELETE,
            &format!("/api/certificates/{}", other.0),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn status_names_the_certificate_the_admin_plane_serves() {
    let app = TestApp::with_admin_tls().await;
    let (_, body) = app.get("/api/status").await;
    assert_eq!(body["data"]["admin"]["tlsCertificate"], "admin");
    let plain = TestApp::new().await;
    let (_, body) = plain.get("/api/status").await;
    assert!(body["data"]["admin"]["tlsCertificate"].is_null());
}
