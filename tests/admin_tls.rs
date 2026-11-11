//! The admin plane served over TLS, against the real binary: the generated
//! `admin` certificate is stored and reused, the plain port speaks only TLS,
//! and the exposure rules accept a non-loopback listener once TLS is on.

mod common;

use std::net::SocketAddr;
use std::process::{Command, Stdio};

use common::console::{printed, start_ok_with_args, token_in, Running};
use common::{free_port, write_config};
use fast_proxy_test_pki::{handshake, request, ClientHello};

fn hello() -> ClientHello {
    ClientHello {
        sni: Some("localhost".into()),
        insecure: true,
        alpn: vec!["http/1.1".into()],
        ..ClientHello::default()
    }
}

async fn get(running: &Running, path: &str, token: Option<&str>) -> String {
    let authorization = token
        .map(|t| format!("Authorization: Bearer {t}\r\n"))
        .unwrap_or_default();
    let raw = format!(
        "GET {path} HTTP/1.1\r\nHost: localhost\r\n{authorization}Connection: close\r\n\r\n"
    );
    request(running.admin, &hello(), raw.as_bytes())
        .await
        .expect("the TLS request completes")
}

fn body_of(answer: &str) -> serde_json::Value {
    let body = answer.split("\r\n\r\n").nth(1).unwrap_or_default();
    // The server may chunk the body; keep the JSON between the outer braces.
    let start = body.find('{').unwrap_or(0);
    let end = body.rfind('}').map_or(body.len(), |at| at + 1);
    serde_json::from_str(&body[start..end]).unwrap_or(serde_json::Value::Null)
}

#[tokio::test]
async fn the_admin_plane_serves_a_generated_stored_certificate_over_https() {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok_with_args(dir.path(), &[], &["--admin-tls"]);
    let token = token_in(&printed(&running)).expect("a bootstrap token");

    let health = get(&running, "/api/health", None).await;
    assert!(health.starts_with("HTTP/1.1 200"), "{health}");
    assert!(get(&running, "/api/status", None)
        .await
        .starts_with("HTTP/1.1 401"));

    let certificates = body_of(&get(&running, "/api/certificates", Some(&token)).await);
    let admin = &certificates["data"][0];
    assert_eq!(admin["name"], "admin");
    assert_eq!(admin["source"], "self_signed");
    assert_eq!(admin["names"], serde_json::json!(["localhost"]));

    let status = body_of(&get(&running, "/api/status", Some(&token)).await);
    assert_eq!(status["data"]["admin"]["tlsCertificate"], "admin");
    assert_eq!(status["data"]["admin"]["protectedChannel"], true);
}

#[tokio::test]
async fn the_same_certificate_is_served_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let first = start_ok_with_args(dir.path(), &[], &["--admin-tls"]);
    let before = handshake(first.admin, &hello()).await.unwrap().leaf_der;
    drop(first);

    let second = start_ok_with_args(dir.path(), &[], &["--admin-tls"]);
    let after = handshake(second.admin, &hello()).await.unwrap().leaf_der;
    assert_eq!(
        before, after,
        "an existing certificate is never regenerated"
    );
}

#[tokio::test]
async fn the_plain_port_does_not_answer_http_when_tls_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let running = start_ok_with_args(dir.path(), &[], &["--admin-tls"]);
    let answer = {
        use std::io::{Read, Write};
        let mut tcp = std::net::TcpStream::connect(running.admin).unwrap();
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        tcp.write_all(b"GET /api/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut buffer = Vec::new();
        let _ = tcp.read_to_end(&mut buffer);
        String::from_utf8_lossy(&buffer).into_owned()
    };
    assert!(!answer.starts_with("HTTP/"), "{answer:?}");
}

#[test]
fn a_non_loopback_listener_is_accepted_once_tls_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let listen = format!("0.0.0.0:{port}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(write_config(dir.path(), free_port(), free_port(), ""))
        .args([
            "--admin-auth",
            "required",
            "--admin-tls",
            "--admin-listen",
            &listen,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut listening = false;
    while std::time::Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::net::TcpStream::connect(addr).is_ok() {
            listening = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        listening,
        "the proxy should start on 0.0.0.0 with admin TLS on"
    );
}
