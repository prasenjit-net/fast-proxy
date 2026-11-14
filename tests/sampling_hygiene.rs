//! Secret hygiene for sampled requests, against the real binary: a request that
//! carries a bearer token, a cookie, an API key, secret query parameters and
//! secret body fields (with bodies captured for the route) is kept, and none of
//! those values is in the sample API, the export, the metrics, the access log or
//! the database file and its write-ahead log. A value that is not secret is kept,
//! so the test cannot pass by storing nothing.

mod common;

use std::path::Path;

use common::*;
use serde_json::{json, Value};

const HOST: &str = "shop.example.com";

/// Values that must never be stored; each begins with `ZQ` so one scan finds any.
const SECRETS: [&str; 9] = [
    "ZQREFERER-token",
    "ZQBEARER-token",
    "ZQCOOKIE-value",
    "ZQAPIKEY-value",
    "ZQQUERY-token",
    "ZQCUSTOM-query",
    "ZQBODY-password",
    "ZQBODY-iban",
    "ZQCUSTOM-header",
];

fn files_with_prefix(dir: &Path, prefix: &str) -> Vec<(String, Vec<u8>)> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                std::fs::read(entry.path()).unwrap(),
            )
        })
        .collect()
}

fn assert_clean(what: &str, text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{what} holds {secret}: {text}");
    }
}

#[test]
fn no_secret_of_a_sampled_request_is_stored_or_shown_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let access_log = dir.path().join("access.log");
    let mut proxy = try_start_with_args(
        dir.path(),
        free_port(),
        free_port(),
        ("", ""),
        &["--access-log", access_log.to_str().unwrap()],
    )
    .unwrap();
    // A slow backend: the request body is read on its way there, so it can be kept.
    let backend = start_slow_backend("ok", std::time::Duration::from_millis(200));
    let (subsystem, _) = create_site(&proxy, "shop", HOST, backend);

    // Extra names to redact, and bodies on for the subsystem.
    let (_, current) = admin(&proxy, "GET", "/api/sampling", None);
    let mut settings = current["data"]["settings"].clone();
    for derived in ["builtInRedaction", "updatedBy", "updatedAtMs"] {
        settings.as_object_mut().unwrap().remove(derived);
    }
    settings["redactHeaders"] = json!(["x-custom-secret"]);
    settings["redactQueryParams"] = json!(["customsecret"]);
    settings["redactBodyFields"] = json!(["iban"]);
    settings["extraHeaders"] = json!(["x-tenant", "authorization", "x-custom-secret"]);
    settings["slowThresholdMs"] = json!(50);
    assert_eq!(admin(&proxy, "PUT", "/api/sampling", Some(settings)).0, 200);
    let (status, body) = admin(
        &proxy,
        "PUT",
        "/api/sampling/overrides",
        Some(json!({ "subsystemId": subsystem, "captureBodies": true })),
    );
    assert_eq!(status, 200, "{body}");

    let payload = json!({
        "name": "ann",
        "password": "ZQBODY-password",
        "iban": "ZQBODY-iban",
        "nested": { "token": "ZQBODY-password" },
    });
    let (status, _) = http_with_headers(
        proxy.data,
        "POST",
        "/login?token=ZQQUERY-token&customsecret=ZQCUSTOM-query&user=ann",
        HOST,
        &[
            ("Authorization", "Bearer ZQBEARER-token"),
            ("Cookie", "session=ZQCOOKIE-value"),
            ("X-Api-Key", "ZQAPIKEY-value"),
            ("X-Custom-Secret", "ZQCUSTOM-header"),
            ("X-Tenant", "acme"),
            (
                "Referer",
                "https://shop.example.com/reset?token=ZQREFERER-token&step=2",
            ),
        ],
        Some(&payload),
    );
    assert_eq!(status, 200);
    assert!(wait_until(|| !admin(&proxy, "GET", "/api/samples", None).1
        ["data"]
        .as_array()
        .unwrap()
        .is_empty()));

    // What the API shows: the list, the detail, the export, the metrics.
    let (_, list) = admin(&proxy, "GET", "/api/samples", None);
    let id = list["data"][0]["id"].clone();
    let (_, detail) = admin(&proxy, "GET", &format!("/api/samples/{id}"), None);
    let detail_text = detail.to_string();
    assert_clean("the sample detail", &detail_text);
    assert_clean("the sample list", &list.to_string());
    let (_, export) = http(proxy.admin, "GET", "/api/samples/export", "localhost", None);
    assert_clean("the export", &export);
    let (_, metrics) = http(proxy.admin, "GET", "/metrics", "localhost", None);
    assert_clean("the metrics", &metrics);

    // Positive controls: ordinary values are kept, secrets are replaced.
    assert!(
        detail_text.contains("acme"),
        "an extra header is kept: {detail_text}"
    );
    assert!(detail_text.contains("user=ann"), "{detail_text}");
    assert!(detail_text.contains("[redacted]"), "{detail_text}");
    let body_text = detail["data"]["request"]["body"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        body_text.contains("\"name\""),
        "the body was captured: {detail_text}"
    );
    assert!(!body_text.contains("ZQ"), "{body_text}");

    // What reached the disk.
    #[cfg(unix)]
    {
        interrupt(&proxy);
        assert_eq!(
            wait_for_exit(&mut proxy, std::time::Duration::from_secs(20)),
            Some(true)
        );
    }
    #[cfg(not(unix))]
    drop(proxy);
    let files = files_with_prefix(&dir.path().join("state"), "fast-proxy.db");
    assert!(!files.is_empty());
    for (name, bytes) in files {
        assert_clean(&name, &String::from_utf8_lossy(&bytes));
    }
    // The access log is the operator's own file of raw request lines, query string
    // included (see docs/security-review-m6.md); it never holds a header or a body.
    let log = std::fs::read_to_string(&access_log).unwrap_or_default();
    assert!(log.contains("/login"), "the request was logged: {log}");
    for secret in SECRETS
        .iter()
        .filter(|secret| !secret.starts_with("ZQQUERY") && !secret.starts_with("ZQCUSTOM-query"))
    {
        assert!(!log.contains(secret), "the access log holds {secret}");
    }
}

#[test]
fn a_sample_without_body_capture_holds_no_body_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    create_site(&proxy, "shop", HOST, free_port());
    let payload: Value = json!({ "note": "ZQBODY-plain-text-not-on-any-list" });
    let (status, _) = http(proxy.data, "POST", "/note", HOST, Some(&payload));
    assert_eq!(status, 502);
    assert!(wait_until(|| !admin(&proxy, "GET", "/api/samples", None).1
        ["data"]
        .as_array()
        .unwrap()
        .is_empty()));
    let (_, list) = admin(&proxy, "GET", "/api/samples", None);
    let id = list["data"][0]["id"].clone();
    let (_, detail) = admin(&proxy, "GET", &format!("/api/samples/{id}"), None);
    assert!(detail["data"]["request"]["body"].is_null(), "{detail}");
    assert!(!detail.to_string().contains("ZQBODY"));
}
