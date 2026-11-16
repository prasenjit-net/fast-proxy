//! ACME against the real binary (M4 exit criteria 1, 7 and 8): turning ACME on
//! for a subsystem through the admin API gets it a certificate from the CA,
//! served over HTTPS without a restart; the CLI shows it and forces a renewal;
//! nothing the proxy wrote, answered or logged holds a private key or the
//! account credentials in clear.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::acme_binary::{config_lines, files_under, trusted_get, turn_acme_on, wait_for_status};
use common::{admin, create_site, free_port, http, start_backend, start_on_free_ports, wait_until};
use fast_proxy_test_acme::TestAcme;

const SHOP: &str = "shop.example.com";
const PATIENCE: Duration = Duration::from_secs(30);

/// Text that would only be on disk if a key or the account credentials leaked.
const SECRET_MARKERS: [&[u8]; 2] = [b"PRIVATE KEY", b"key_pkcs8"];

struct Ran {
    code: i32,
    out: String,
}

fn cli(dir: &Path, args: &[&str]) -> Ran {
    let output = Command::new(env!("CARGO_BIN_EXE_fast-proxy"))
        .arg("--config")
        .arg(dir.join("config.toml"))
        .args(args)
        .env_remove("FAST_PROXY_TOKEN")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    Ran {
        code: output.status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&output.stdout).to_string()
            + &String::from_utf8_lossy(&output.stderr),
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn a_subsystem_gets_a_trusted_certificate_the_cli_shows_and_renews_it_and_no_secret_leaks() {
    let runtime = runtime();
    let ca = runtime.block_on(TestAcme::start());
    let dir = tempfile::tempdir().unwrap();
    let https_port = free_port();
    let extra = config_lines(&ca, https_port, "");
    let proxy = start_on_free_ports(dir.path(), &extra, "");
    ca.set_challenge_targets(vec![proxy.data]);
    let backend = start_backend("shop-backend");
    let (id, _) = create_site(&proxy, "shop", SHOP, backend);
    let https = ([127, 0, 0, 1], https_port).into();
    assert!(trusted_get(&runtime, https, SHOP, &ca.issuer_pem).starts_with("handshake failed"));

    turn_acme_on(&proxy, id, "shop");
    let status = wait_for_status(&proxy, "shop", PATIENCE, |s| s["state"] == "valid");
    assert_eq!(status["certificateName"], "acme-shop", "{status}");
    let answer = trusted_get(&runtime, https, SHOP, &ca.issuer_pem);
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.contains("shop-backend"), "{answer}");

    let shown = cli(
        dir.path(),
        &[
            "acme",
            "status",
            "--url",
            &format!("http://{}", proxy.admin),
        ],
    );
    assert_eq!(shown.code, 0, "{}", shown.out);
    assert!(shown.out.contains("account: registered"), "{}", shown.out);
    assert!(
        shown.out.contains("shop: valid [shop.example.com]"),
        "{}",
        shown.out
    );
    assert!(shown.out.contains("(this instance)"), "{}", shown.out);

    let renewed = cli(
        dir.path(),
        &[
            "acme",
            "renew",
            "shop",
            "--url",
            &format!("http://{}", proxy.admin),
        ],
    );
    assert_eq!(renewed.code, 0, "{}", renewed.out);
    assert!(
        wait_until_within(PATIENCE, || ca.issued() == 2),
        "the forced renewal was ordered"
    );
    wait_for_status(&proxy, "shop", PATIENCE, |s| s["state"] == "valid");
    let missing = cli(
        dir.path(),
        &[
            "acme",
            "renew",
            "nope",
            "--url",
            &format!("http://{}", proxy.admin),
        ],
    );
    assert_eq!(missing.code, 1, "{}", missing.out);

    // Hygiene: no key or credential in any file, API answer or metric.
    for (path, bytes) in files_under(dir.path()) {
        for marker in SECRET_MARKERS {
            assert!(
                !bytes.windows(marker.len()).any(|w| w == marker),
                "{path} holds {:?}",
                String::from_utf8_lossy(marker)
            );
        }
    }
    for path in [
        "/api/acme",
        "/api/certificates",
        "/api/audit?limit=200",
        "/metrics",
    ] {
        let (_, text) = http(proxy.admin, "GET", path, "localhost", None);
        for marker in SECRET_MARKERS {
            let marker = String::from_utf8_lossy(marker);
            assert!(!text.contains(marker.as_ref()), "{path} holds {marker}");
        }
    }
    let (_, metrics) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    assert!(metrics.contains(r#"fastproxy_acme_orders_total{outcome="issued",subsystem="shop"} 1"#));
    assert!(
        metrics.contains(r#"fastproxy_acme_orders_total{outcome="renewed",subsystem="shop"} 1"#)
    );
}

#[test]
fn with_acme_off_the_switch_is_refused_and_the_cli_says_acme_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let https_port = free_port();
    let proxy = start_on_free_ports(
        dir.path(),
        &format!("[data_plane.tls]\nenabled = true\nlisten = \"127.0.0.1:{https_port}\"\n"),
        "",
    );
    let (id, _) = create_site(&proxy, "shop", SHOP, start_backend("shop"));
    let (status, body) = admin(
        &proxy,
        "PUT",
        &format!("/api/subsystems/{id}"),
        Some(
            serde_json::json!({ "name": "shop", "https": { "enabled": true, "acmeEnabled": true } }),
        ),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["fields"][0]["field"], "https.acmeEnabled");
    let shown = cli(
        dir.path(),
        &[
            "acme",
            "status",
            "--url",
            &format!("http://{}", proxy.admin),
        ],
    );
    assert_eq!(shown.code, 0, "{}", shown.out);
    assert!(shown.out.contains("ACME is off"), "{}", shown.out);
}

#[test]
fn acme_without_tls_or_an_allowlist_refuses_to_start() {
    let dir = tempfile::tempdir().unwrap();
    for config in [
        "[data_plane.acme]\nenabled = true\nallowed_domains = [\"*.example.com\"]\n".to_string(),
        "[data_plane.tls]\nenabled = true\n[data_plane.acme]\nenabled = true\n".to_string(),
    ] {
        let started = common::try_start(dir.path(), free_port(), free_port(), &config, "");
        assert!(started.is_err(), "{config}");
    }
}

fn wait_until_within(limit: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // One last look with the shared helper's shorter wait.
    wait_until(condition)
}
