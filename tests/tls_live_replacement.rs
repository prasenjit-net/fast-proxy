//! Certificates change under load (M3): clients keep making new HTTPS
//! connections and requests while certificates are uploaded, replaced and
//! deleted over and over and another subsystem toggles HTTPS, with invalid
//! uploads mixed in. For a name that stays enabled no handshake may fail, every
//! handshake must get a certificate that was valid at some point during it, and
//! a connection that is already open keeps working.

mod common;

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use common::tls_stack::{Options, Stack};
use fast_proxy_config_service::CertificateChange;
use fast_proxy_model::HttpsPolicy;
use fast_proxy_test_pki::{connect, handshake, ClientHello, Issued, LeafSpec, TestCa};
use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;

fn leaf_of(issued: &Issued) -> Vec<u8> {
    CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
        .next()
        .unwrap()
        .unwrap()
        .as_ref()
        .to_vec()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshakes_never_fail_while_certificates_and_https_settings_change() {
    let stack = Arc::new(Stack::start(Options::default()).await);
    let ca = Arc::new(TestCa::new("ca"));
    let shop = stack.add_site("shop", "shop.example.com").await;
    let blog = stack.add_site("blog", "blog.example.com").await;
    let first = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let shop_cert = stack
        .service
        .upload_certificate("shop", &first.chain_pem, &first.key_pem)
        .await
        .unwrap()
        .value
        .id;
    stack
        .service
        .set_subsystem_https(
            shop,
            HttpsPolicy {
                enabled: true,
                certificate_id: Some(shop_cert),
                ..HttpsPolicy::default()
            },
        )
        .await
        .unwrap();
    // Every leaf that was ever valid for the shop, recorded before it is published.
    let valid: Arc<Mutex<HashSet<Vec<u8>>>> =
        Arc::new(Mutex::new(HashSet::from([leaf_of(&first)])));

    // A connection opened now must keep working through every change.
    let hello = ClientHello {
        sni: Some("shop.example.com".into()),
        trust_pem: vec![ca.pem.clone()],
        alpn: vec!["h2".into()],
        ..ClientHello::default()
    };
    let (tls, _) = connect(stack.https, &hello).await.unwrap();
    let (mut established, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(connection);

    let stop = Arc::new(AtomicBool::new(false));
    let (handshakes, failures, foreign) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let mut clients = Vec::new();
    for _ in 0..6 {
        let (stack, ca, valid, stop) = (stack.clone(), ca.clone(), valid.clone(), stop.clone());
        let (handshakes, failures, foreign) =
            (handshakes.clone(), failures.clone(), foreign.clone());
        clients.push(tokio::spawn(async move {
            let client = ClientHello {
                sni: Some("shop.example.com".into()),
                trust_pem: vec![ca.pem.clone()],
                ..ClientHello::default()
            };
            while !stop.load(Ordering::Relaxed) {
                match handshake(stack.https, &client).await {
                    Ok(negotiated) => {
                        handshakes.fetch_add(1, Ordering::Relaxed);
                        if !valid.lock().unwrap().contains(&negotiated.leaf_der) {
                            foreign.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        failures.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // The writer: renewals, uploads and deletions, HTTPS toggles, and invalid uploads.
    for round in 0..25 {
        let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
        valid.lock().unwrap().insert(leaf_of(&renewed));
        stack
            .service
            .replace_certificate(
                shop_cert,
                CertificateChange {
                    cert_pem: Some(renewed.chain_pem.clone()),
                    key_pem: Some(renewed.key_pem.clone()),
                    ..CertificateChange::default()
                },
            )
            .await
            .unwrap();

        let extra = ca.issue(&LeafSpec::new(&["blog.example.com"]));
        let name = format!("blog-{round}");
        let uploaded = stack
            .service
            .upload_certificate(&name, &extra.chain_pem, &extra.key_pem)
            .await
            .unwrap()
            .value
            .id;
        let attached = HttpsPolicy {
            enabled: round % 2 == 0,
            certificate_id: Some(uploaded),
            ..HttpsPolicy::default()
        };
        stack
            .service
            .set_subsystem_https(blog, attached)
            .await
            .unwrap();

        let invalid = stack
            .service
            .upload_certificate("invalid", "garbage", &renewed.key_pem)
            .await;
        assert!(
            invalid.is_err(),
            "an invalid upload is rejected and changes nothing"
        );

        let detached = HttpsPolicy {
            enabled: false,
            ..HttpsPolicy::default()
        };
        stack
            .service
            .set_subsystem_https(blog, detached)
            .await
            .unwrap();
        stack.service.delete_certificate(uploaded).await.unwrap();

        // The open connection is untouched by all of this.
        let request = Request::builder()
            .uri("https://shop.example.com/")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = established.send_request(request).await.unwrap();
        assert_eq!(response.status(), 200, "round {round}");
        response.into_body().collect().await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    stop.store(true, Ordering::Relaxed);
    for client in clients {
        client.await.unwrap();
    }
    let total = handshakes.load(Ordering::Relaxed);
    assert!(total > 100, "the clients must have run: {total} handshakes");
    assert_eq!(
        failures.load(Ordering::Relaxed),
        0,
        "no handshake for an enabled name may fail"
    );
    assert_eq!(
        foreign.load(Ordering::Relaxed),
        0,
        "every certificate served was one that had been published"
    );
}
