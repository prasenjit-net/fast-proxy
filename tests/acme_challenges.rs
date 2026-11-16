//! ACME through the real data plane listeners (M4 PR2): a published HTTP-01
//! challenge is answered on the plain listener ahead of the HTTPS redirect and
//! over HTTPS too, and a certificate the ACME path stores is presented by the
//! very next handshake, without a restart. The protocol client and the CA come
//! in PR3; here the challenge and the issued chain are handed in directly.

mod common;

use common::tls_stack::{contains, Options, Stack};
use fast_proxy_config_service::{ChangeContext, IssuedCertificate};
use fast_proxy_model::{
    AcmeChallenge, AcmeKeyType, AcmeToken, HttpsPolicy, RedirectMode, SubsystemId,
};
use fast_proxy_test_pki::{request, LeafSpec, TestCa};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const SHOP: &str = "shop.example.com";
const PATH: &str = "/.well-known/acme-challenge/tok-1";

async fn plain(stack: &Stack, path: &str) -> String {
    let mut stream = TcpStream::connect(stack.http).await.unwrap();
    stream.set_nodelay(true).unwrap();
    let raw = format!("GET {path} HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n\r\n");
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut answer = String::new();
    stream.read_to_string(&mut answer).await.unwrap();
    answer
}

/// The shop served with an uploaded certificate, redirecting plain HTTP with a
/// 308, and with its ACME switch on.
async fn redirecting_acme_shop(stack: &Stack, ca: &TestCa) -> SubsystemId {
    let site = stack.add_site("shop", SHOP).await;
    let certificate = stack.serve_with(site, "manual", ca, &[SHOP]).await;
    let https = HttpsPolicy {
        enabled: true,
        certificate_id: Some(certificate),
        redirect: RedirectMode::Permanent308,
        acme_enabled: true,
        ..HttpsPolicy::default()
    };
    stack
        .service
        .set_subsystem_https(&ChangeContext::system(), site, https)
        .await
        .unwrap();
    site
}

async fn publish(stack: &Stack, site: SubsystemId) {
    let challenge = AcmeChallenge::new(
        AcmeToken::parse("tok-1").unwrap(),
        site,
        vec![SHOP.into()],
        "tok-1.thumbprint".into(),
        i64::MAX,
    )
    .unwrap();
    stack
        .service
        .publish_acme_challenges(site, vec![challenge])
        .await
        .unwrap();
}

fn acme_options() -> Options {
    Options {
        acme: true,
        ..Options::default()
    }
}

#[tokio::test]
async fn a_challenge_is_answered_on_plain_http_while_everything_else_is_redirected() {
    let stack = Stack::start(acme_options()).await;
    let ca = TestCa::new("ca");
    let site = redirecting_acme_shop(&stack, &ca).await;
    assert!(plain(&stack, "/").await.starts_with("HTTP/1.1 308"));
    assert!(
        plain(&stack, PATH).await.starts_with("HTTP/1.1 308"),
        "no challenge yet"
    );

    publish(&stack, site).await;
    let answer = plain(&stack, PATH).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("\r\n\r\ntok-1.thumbprint"), "{answer}");
    assert!(plain(&stack, "/").await.starts_with("HTTP/1.1 308"));

    stack.service.clear_acme_challenges(site).await.unwrap();
    assert!(
        plain(&stack, PATH).await.starts_with("HTTP/1.1 308"),
        "gone again"
    );
}

#[tokio::test]
async fn a_challenge_is_answered_over_https_too() {
    let stack = Stack::start(acme_options()).await;
    let ca = TestCa::new("ca");
    let site = redirecting_acme_shop(&stack, &ca).await;
    publish(&stack, site).await;
    let raw = format!("GET {PATH} HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n\r\n");
    let answer = request(stack.https, &stack.hello(SHOP, &ca), raw.as_bytes())
        .await
        .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.ends_with("tok-1.thumbprint"), "{answer}");
}

#[tokio::test]
async fn a_stored_acme_certificate_is_presented_by_the_next_handshake() {
    let stack = Stack::start(acme_options()).await;
    let manual_ca = TestCa::new("manual ca");
    let site = redirecting_acme_shop(&stack, &manual_ca).await;
    let before = stack.leaf_seen(SHOP).await;

    let names = vec![SHOP.to_string()];
    let order = fast_proxy_tls::generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
    let acme_ca = TestCa::new("acme ca");
    let chain_pem = acme_ca
        .sign_csr(&order.csr_der, &LeafSpec::new(&[]))
        .unwrap();
    let leaf_der = fast_proxy_tls::parse_chain(&chain_pem).unwrap().remove(0);
    stack
        .service
        .store_acme_certificate(IssuedCertificate {
            subsystem: site,
            chain_pem: chain_pem.clone(),
            key: order.key,
            names,
        })
        .await
        .unwrap();

    let after = stack.leaf_seen(SHOP).await;
    assert_ne!(after, before, "the new certificate is served");
    assert!(contains(&after, leaf_der.as_ref()), "it is the ACME leaf");
    // And a client trusting only the ACME CA completes a request.
    let raw = format!("GET / HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n\r\n");
    let answer = request(stack.https, &stack.hello(SHOP, &acme_ca), raw.as_bytes())
        .await
        .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}
