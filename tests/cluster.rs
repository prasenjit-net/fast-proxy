//! Two real `fast-proxy` processes sharing one database file: a change made
//! through one instance's admin API reaches the other within the poll interval
//! (M3 cluster readiness). Both use the same master key file, as a cluster must.

mod common;

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{free_port, try_start_with_args, Proxy};
use fast_proxy_test_pki::{handshake, request, ClientHello, LeafSpec, TestCa};
use serde_json::json;

struct Instance {
    proxy: Proxy,
    https: SocketAddr,
}

/// Starts one instance on its own ports, sharing `shared` (database and master
/// key) with the others, retrying when a parallel test took a port.
fn start_instance(own_dir: &Path, shared: &Path) -> Instance {
    for _ in 0..5 {
        let https_port = free_port();
        let extra = format!(
            "database = {:?}\nmaster_key_file = {:?}\nconfig_poll_interval = \"200ms\"\n",
            shared.join("shared.db"),
            shared.join("master.key")
        );
        let tail =
            format!("\n[data_plane.tls]\nenabled = true\nlisten = \"127.0.0.1:{https_port}\"\n");
        if let Ok(proxy) =
            try_start_with_args(own_dir, free_port(), free_port(), (&extra, &tail), &[])
        {
            return Instance {
                proxy,
                https: ([127, 0, 0, 1], https_port).into(),
            };
        }
    }
    panic!("an instance would not start on any of 5 port sets");
}

/// Retries `check` until it holds, for at most 30 s (generous for slow CI).
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("never happened: {what}");
}

fn trusting(sni: &str, ca: &TestCa) -> ClientHello {
    ClientHello {
        sni: Some(sni.into()),
        trust_pem: vec![ca.pem.clone()],
        ..ClientHello::default()
    }
}

async fn leaf_of(address: SocketAddr, sni: &str) -> Option<Vec<u8>> {
    let hello = ClientHello {
        sni: Some(sni.into()),
        insecure: true,
        ..ClientHello::default()
    };
    handshake(address, &hello).await.ok().map(|n| n.leaf_der)
}

#[tokio::test]
async fn a_change_on_one_process_reaches_the_other() {
    let shared = tempfile::tempdir().unwrap();
    let (dir_a, dir_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = start_instance(dir_a.path(), shared.path());
    let b = start_instance(dir_b.path(), shared.path());
    let backend = common::start_backend("shop-backend");

    // Configure everything through A only.
    let (site, _) = common::create_site(&a.proxy, "shop", "shop.example.com", backend);
    eventually("B routes the new subsystem", || {
        matches!(common::through_proxy(&b.proxy, "shop.example.com", "/x"),
            (200, body) if body == "shop-backend /x")
    });

    let ca = TestCa::new("shop ca");
    let issued = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, uploaded) = common::admin(
        &a.proxy,
        "POST",
        &format!("/api/subsystems/{site}/certificate"),
        Some(json!({ "name": "shop", "certPem": issued.chain_pem, "keyPem": issued.key_pem })),
    );
    assert_eq!(status, 201, "{uploaded}");
    let (status, body) = common::admin(
        &a.proxy,
        "PUT",
        &format!("/api/subsystems/{site}"),
        Some(json!({ "name": "shop", "https": {
            "enabled": true, "certificateId": uploaded["data"]["id"] } })),
    );
    assert_eq!(status, 200, "{body}");

    let mut served = false;
    for _ in 0..300 {
        if handshake(b.https, &trusting("shop.example.com", &ca))
            .await
            .is_ok()
        {
            served = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(served, "B serves the certificate uploaded through A");
    let answer = request(
        b.https,
        &trusting("shop.example.com", &ca),
        b"GET /cart HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(answer.contains("shop-backend /cart"), "{answer}");

    // A renewal on A replaces the certificate on B.
    let before = leaf_of(b.https, "shop.example.com").await.unwrap();
    let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, body) = common::admin(
        &a.proxy,
        "PUT",
        &format!("/api/certificates/{}", uploaded["data"]["id"]),
        Some(json!({ "certPem": renewed.chain_pem, "keyPem": renewed.key_pem })),
    );
    assert_eq!(status, 200, "{body}");
    let mut changed = false;
    for _ in 0..300 {
        if leaf_of(b.https, "shop.example.com")
            .await
            .is_some_and(|leaf| leaf != before)
        {
            changed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(changed, "the renewal reached B");
    let (_, tls) = common::admin(&b.proxy, "GET", "/api/tls", None);
    assert_eq!(tls["data"]["vault"], "ok");
}
