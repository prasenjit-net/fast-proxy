//! The protocol client against the in-process test CA: registration, restore,
//! an order validated by a real fetch, our own CSR finalized, the chain
//! downloaded; and the CA's refusals mapped to failure kinds.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fast_proxy_acme::{AcmeClient, FailureKind};
use fast_proxy_model::AcmeKeyType;
use fast_proxy_test_acme::TestAcme;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Answers = Arc<Mutex<HashMap<String, String>>>;

/// A tiny HTTP server answering challenges from `answers`.
async fn challenge_server(answers: Answers) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let answers = answers.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]);
                let token = head
                    .split_whitespace()
                    .nth(1)
                    .and_then(|p| p.strip_prefix("/.well-known/acme-challenge/"))
                    .unwrap_or("")
                    .to_string();
                let reply = match answers.lock().unwrap().get(&token) {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ),
                    None => {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            .into()
                    }
                };
                let _ = stream.write_all(reply.as_bytes()).await;
            });
        }
    });
    addr
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

#[tokio::test]
async fn an_order_is_validated_by_a_real_fetch_finalized_and_downloaded() {
    let ca = TestAcme::start().await;
    let answers: Answers = Arc::default();
    ca.set_challenge_targets(vec![challenge_server(answers.clone()).await]);
    let client = AcmeClient::new(ca.directory.clone(), Some(ca.directory_ca.clone()));

    let (_, credentials) = client.register("ops@example.com").await.unwrap();
    let session = client.restore(&credentials).await.unwrap();
    assert_eq!(ca.accounts(), 1, "restoring registers nothing");

    let names = vec![
        "shop.example.com".to_string(),
        "www.shop.example.com".to_string(),
    ];
    let mut order = session.new_order(&names).await.unwrap();
    for challenge in order.challenges() {
        assert!(challenge
            .key_authorization
            .starts_with(&format!("{}.", challenge.token)));
        answers
            .lock()
            .unwrap()
            .insert(challenge.token.clone(), challenge.key_authorization.clone());
    }
    order.set_ready().await.unwrap();
    order.wait_ready(Duration::from_secs(20)).await.unwrap();
    let key = fast_proxy_tls::generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
    let chain = order
        .finalize(&key.csr_der, Duration::from_secs(20))
        .await
        .unwrap();
    fast_proxy_tls::validate_issued(&chain, key.key, &names, now_ms()).unwrap();

    let validations = ca.validations();
    assert_eq!(validations.len(), 2);
    assert!(validations.iter().all(|v| v.ok));
    assert_eq!(ca.issued(), 1);
}

#[tokio::test]
async fn an_unanswered_challenge_fails_with_an_unauthorized_problem() {
    let ca = TestAcme::start().await;
    ca.set_challenge_targets(vec![challenge_server(Arc::default()).await]);
    let client = AcmeClient::new(ca.directory.clone(), Some(ca.directory_ca.clone()));
    let (session, _) = client.register("").await.unwrap();
    let mut order = session
        .new_order(&["shop.example.com".to_string()])
        .await
        .unwrap();
    order.set_ready().await.unwrap();
    let err = order.wait_ready(Duration::from_secs(20)).await.unwrap_err();
    assert_eq!(err.kind, FailureKind::Permanent, "{err}");
    assert!(err.detail.starts_with("unauthorized:"), "{err}");
    assert!(!ca.validations()[0].ok);
}

#[tokio::test]
async fn a_rate_limited_order_is_reported_as_such() {
    let ca = TestAcme::start().await;
    ca.set_rate_limited(true);
    let client = AcmeClient::new(ca.directory.clone(), Some(ca.directory_ca.clone()));
    let (session, _) = client.register("").await.unwrap();
    let err = session
        .new_order(&["shop.example.com".to_string()])
        .await
        .err()
        .unwrap();
    assert_eq!(
        err.kind,
        FailureKind::RateLimited { retry_at_ms: None },
        "{err}"
    );
}

#[tokio::test]
async fn a_directory_that_is_not_trusted_fails_transiently() {
    let ca = TestAcme::start().await;
    let client = AcmeClient::new(ca.directory.clone(), None);
    let err = client.register("").await.err().unwrap();
    assert_eq!(err.kind, FailureKind::Transient, "{err}");
}
