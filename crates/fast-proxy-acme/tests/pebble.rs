//! The protocol client against a real ACME server, Pebble (spike G2 of M4,
//! and the Pebble CI job). Ignored unless asked for:
//!
//! ```sh
//! # in a checkout of letsencrypt/pebble (for its test/ directory)
//! pebble-challtestsrv -defaultIPv4 127.0.0.1 -defaultIPv6 "" -http01 "" -https01 "" -tlsalpn01 "" -doh "" &
//! PEBBLE_VA_NOSLEEP=1 PEBBLE_WFE_NONCEREJECT=0 pebble -config test/config/pebble-config.json -dnsserver 127.0.0.1:8053 &
//! FAST_PROXY_PEBBLE_CA=$PWD/test/certs/pebble.minica.pem \
//!   cargo test --manifest-path <fast-proxy>/Cargo.toml -p fast-proxy-acme --test pebble -- --ignored --test-threads=1
//! ```
//!
//! The test answers HTTP-01 itself on Pebble's `httpPort` (5002), so it checks
//! registration, restoring an account, ordering, challenge validation,
//! finalizing with our own CSR and downloading the chain.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fast_proxy_acme::{AcmeClient, FailureKind};
use fast_proxy_model::AcmeKeyType;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

type Answers = Arc<Mutex<HashMap<String, String>>>;

fn client() -> AcmeClient {
    let ca =
        std::env::var("FAST_PROXY_PEBBLE_CA").expect("FAST_PROXY_PEBBLE_CA names Pebble's root");
    let directory = std::env::var("FAST_PROXY_PEBBLE_DIRECTORY")
        .unwrap_or_else(|_| "https://localhost:14000/dir".into());
    AcmeClient::new(directory, Some(ca.into()))
}

/// Answers `GET /.well-known/acme-challenge/<token>` from `answers`.
async fn serve_challenges(answers: Answers) {
    let port: u16 = std::env::var("FAST_PROXY_PEBBLE_HTTP_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(5002);
    let listener = TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], port)))
        .await
        .unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let answers = answers.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]);
                let token = head
                    .split_whitespace()
                    .nth(1)
                    .and_then(|path| path.strip_prefix("/.well-known/acme-challenge/"))
                    .unwrap_or("");
                let body = answers.lock().unwrap().get(token).cloned();
                let reply = match body {
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
}

#[tokio::test]
#[ignore = "needs a running Pebble; see the module docs"]
async fn an_order_is_validated_finalized_and_downloaded_from_pebble() {
    let answers: Answers = Arc::default();
    serve_challenges(answers.clone()).await;
    let client = client();

    let (_, credentials) = client.register("ops@example.com").await.unwrap();
    let session = client.restore(&credentials).await.unwrap();

    let names = vec![
        "spike.example.com".to_string(),
        "www.spike.example.com".to_string(),
    ];
    let mut order = session.new_order(&names).await.unwrap();
    assert_eq!(order.challenges().len(), 2);
    for challenge in order.challenges() {
        assert!(names.contains(&challenge.name));
        assert!(challenge
            .key_authorization
            .starts_with(&format!("{}.", challenge.token)));
        answers
            .lock()
            .unwrap()
            .insert(challenge.token.clone(), challenge.key_authorization.clone());
    }
    order.set_ready().await.unwrap();
    order.wait_ready(Duration::from_secs(60)).await.unwrap();

    let key = fast_proxy_tls::generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
    let chain = order
        .finalize(&key.csr_der, Duration::from_secs(60))
        .await
        .unwrap();
    let issued = fast_proxy_tls::validate_issued(&chain, key.key, &names, now_ms())
        .expect("the chain matches our key and names");
    assert_eq!(issued.metadata.names.len(), 2);
}

#[tokio::test]
#[ignore = "needs a running Pebble; see the module docs"]
async fn a_challenge_nobody_answers_fails_with_the_cas_problem() {
    serve_challenges(Arc::default()).await;
    let client = client();
    let (session, _) = client.register("").await.unwrap();
    let mut order = session
        .new_order(&["unanswered.example.com".to_string()])
        .await
        .unwrap();
    order.set_ready().await.unwrap();
    let err = order.wait_ready(Duration::from_secs(60)).await.unwrap_err();
    assert_eq!(err.kind, FailureKind::Permanent, "{err}");
    assert!(err.detail.starts_with("unauthorized:"), "{err}");
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
