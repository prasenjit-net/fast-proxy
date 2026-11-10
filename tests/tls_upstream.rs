//! TLS to upstreams (M3): a group marked TLS is reached over TLS, verified
//! against an uploaded CA bundle (or not at all), and a certificate the proxy
//! does not trust is refused. The proxy under test is the real data plane.

mod common;

use fast_proxy_config_service::ChangeContext;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::time::Duration;

use common::tls_stack::{tls_backend, Options, Stack};
use fast_proxy_model::UpstreamTls;
use fast_proxy_test_pki::TestCa;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// One request to the plain-HTTP listener; returns the status and body.
async fn through(stack: &Stack, host: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(stack.http).await.unwrap();
    let raw = format!("GET /x HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut text = String::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_string(&mut text))
        .await
        .expect("an answer within 20 s")
        .unwrap();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

fn tls(ca: &str) -> UpstreamTls {
    UpstreamTls {
        enabled: true,
        ca: ca.into(),
        ..UpstreamTls::default()
    }
}

async fn bundle(stack: &Stack, name: &str, ca: &TestCa) -> i64 {
    stack
        .service
        .upload_ca_bundle(&ChangeContext::system(), name, &ca.pem)
        .await
        .unwrap()
        .value
}

#[tokio::test]
async fn a_tls_upstream_with_a_trusted_uploaded_ca_is_proxied() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("corp root");
    let (backend, served) = tls_backend(&ca, &["backend.internal"]).await;
    bundle(&stack, "corp", &ca).await;
    let mut settings = tls("corp");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("shop", "shop.example.com", &[backend], settings)
        .await;

    let (status, body) = through(&stack, "shop.example.com").await;
    assert_eq!((status, body.as_str()), (200, "tls-ok"));
    assert_eq!(served.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_untrusted_certificate_is_refused_with_a_502_and_a_counted_failure() {
    let stack = Stack::start(Options::default()).await;
    let backend_ca = TestCa::new("backend ca");
    let other_ca = TestCa::new("someone else");
    let (backend, served) = tls_backend(&backend_ca, &["backend.internal"]).await;
    bundle(&stack, "other", &other_ca).await;
    let mut settings = tls("other");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("shop", "shop.example.com", &[backend], settings)
        .await;

    assert_eq!(through(&stack, "shop.example.com").await.0, 502);
    assert_eq!(
        served.load(Ordering::SeqCst),
        0,
        "no request reached the backend"
    );
    stack
        .metric("fastproxy_upstream_errors_total{kind=\"connect\",subsystem=\"shop\"}")
        .await;
}

#[tokio::test]
async fn without_a_named_bundle_the_os_trust_store_decides_and_does_not_know_the_test_ca() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("corp root");
    let (backend, _) = tls_backend(&ca, &["backend.internal"]).await;
    let mut settings = tls("");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("shop", "shop.example.com", &[backend], settings)
        .await;
    assert_eq!(through(&stack, "shop.example.com").await.0, 502);
}

#[tokio::test]
async fn a_name_mismatch_is_refused_and_an_override_name_fixes_an_ip_endpoint() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("corp root");
    let (backend, _) = tls_backend(&ca, &["backend.internal"]).await;
    bundle(&stack, "corp", &ca).await;

    // The endpoint is 127.0.0.1 and the certificate is for a name: refused.
    stack
        .add_site_to("shop", "shop.example.com", &[backend], tls("corp"))
        .await;
    assert_eq!(through(&stack, "shop.example.com").await.0, 502);

    // The same endpoint with the override name verifies.
    let mut settings = tls("corp");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("blog", "blog.example.com", &[backend], settings)
        .await;
    assert_eq!(through(&stack, "blog.example.com").await.0, 200);
}

#[tokio::test]
async fn turning_verification_off_accepts_an_untrusted_certificate() {
    let stack = Stack::start(Options::default()).await;
    let (backend, _) = tls_backend(&TestCa::new("anyone"), &["whatever.internal"]).await;
    let settings = UpstreamTls {
        enabled: true,
        verify: false,
        ..UpstreamTls::default()
    };
    stack
        .add_site_to("shop", "shop.example.com", &[backend], settings)
        .await;
    assert_eq!(through(&stack, "shop.example.com").await.0, 200);
}

#[tokio::test]
async fn a_replaced_bundle_takes_effect_for_new_connections() {
    let stack = Stack::start(Options::default()).await;
    let (old, new) = (TestCa::new("old root"), TestCa::new("new root"));
    let (backend, _) = tls_backend(&new, &["backend.internal"]).await;
    let id = bundle(&stack, "corp", &old).await;
    let mut settings = tls("corp");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("shop", "shop.example.com", &[backend], settings)
        .await;

    assert_eq!(
        through(&stack, "shop.example.com").await.0,
        502,
        "the old bundle does not trust it"
    );
    stack
        .service
        .replace_ca_bundle(&ChangeContext::system(), id, &new.pem)
        .await
        .unwrap();
    assert_eq!(
        through(&stack, "shop.example.com").await.0,
        200,
        "the new bundle does"
    );
}

#[tokio::test]
async fn a_retry_moves_from_a_dead_endpoint_to_the_tls_one() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("corp root");
    let (backend, served) = tls_backend(&ca, &["backend.internal"]).await;
    bundle(&stack, "corp", &ca).await;
    let dead: SocketAddr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let mut settings = tls("corp");
    settings.server_name = "backend.internal".into();
    stack
        .add_site_to("shop", "shop.example.com", &[dead, backend], settings)
        .await;
    for _ in 0..6 {
        assert_eq!(through(&stack, "shop.example.com").await.0, 200);
    }
    assert_eq!(served.load(Ordering::SeqCst), 6);
}
