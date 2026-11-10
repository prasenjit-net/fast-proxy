//! HTTP-level tests of the certificate, CA bundle, TLS status and subsystem
//! HTTPS endpoints: success, field errors, 403 for the loopback rule, 404,
//! 409, and proof that a write is live in the data plane's snapshot and that no
//! response, event or metric ever carries key material.

use axum::http::{Method, StatusCode};
use fast_proxy_routing::SniLookup;
use fast_proxy_test_pki::{Issued, LeafSpec, TestCa};
use serde_json::{json, Value};

use crate::test_support::TestApp;

const REMOTE: &str = "203.0.113.9:4000";

fn issue(ca: &TestCa, names: &[&str]) -> Issued {
    ca.issue(&LeafSpec::new(names))
}

fn upload_body(name: &str, issued: &Issued) -> Value {
    json!({ "name": name, "certPem": issued.chain_pem, "keyPem": issued.key_pem })
}

/// The base64 body of the key, which must never be echoed.
fn key_body(issued: &Issued) -> String {
    issued
        .key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect()
}

async fn site(app: &TestApp, name: &str, host: &str) -> i64 {
    let (status, body) = app.post("/api/subsystems", json!({ "name": name })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/hosts"),
            json!({ "pattern": host }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    id
}

fn fields(body: &Value) -> Vec<String> {
    body["error"]["fields"]
        .as_array()
        .map(|all| {
            all.iter()
                .map(|f| f["field"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn serves(app: &TestApp, name: &str) -> bool {
    let snapshot = app.config.snapshot_holder().load();
    matches!(
        snapshot.sni_lookup(name),
        SniLookup::Subsystem { https, .. } if https.certificate.is_some()
    )
}

#[tokio::test]
async fn an_upload_returns_metadata_only_and_is_listed() {
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let issued = issue(&ca, &["shop.example.com", "*.wild.example.com"]);

    let (status, body) = app
        .post("/api/certificates", upload_body("shop", &issued))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let cert = &body["data"];
    assert_eq!(cert["name"], "shop");
    assert_eq!(cert["source"], "uploaded");
    assert_eq!(
        cert["names"],
        json!(["shop.example.com", "*.wild.example.com"])
    );
    assert_eq!(cert["state"], "ok");
    assert_eq!(cert["usedBy"], json!([]));
    assert_eq!(cert["fingerprint"].as_str().unwrap().len(), 64);
    assert!(cert["secondsLeft"].as_i64().unwrap() > 80 * 86_400);

    let (status, listed) = app.get("/api/certificates").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["data"].as_array().unwrap().len(), 1);
    for response in [body.to_string(), listed.to_string()] {
        assert!(
            !response.contains(&key_body(&issued)),
            "no key in a response"
        );
        assert!(!response.contains("PRIVATE KEY") && !response.contains("keyPem"));
        assert!(
            !response.contains("BEGIN CERTIFICATE"),
            "not even the chain"
        );
    }
}

#[tokio::test]
async fn invalid_uploads_name_their_field_and_change_nothing() {
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let good = issue(&ca, &["a.example.com"]);
    let other = issue(&ca, &["b.example.com"]);
    let expired = ca.issue(&LeafSpec::new(&["a.example.com"]).expiring_in_days(-2));
    let version = app.get("/api/certificates").await.1["version"].clone();

    let cases = [
        (
            json!({ "name": "x", "certPem": "nope SECRET", "keyPem": good.key_pem }),
            "certPem",
        ),
        (
            json!({ "name": "x", "certPem": good.cert_pem, "keyPem": "nope SECRET" }),
            "keyPem",
        ),
        (
            json!({ "name": "x", "certPem": good.cert_pem, "keyPem": other.key_pem }),
            "keyPem",
        ),
        (
            json!({ "name": "x", "certPem": expired.cert_pem, "keyPem": expired.key_pem }),
            "certPem",
        ),
        (
            json!({ "name": "bad name!", "certPem": good.cert_pem, "keyPem": good.key_pem }),
            "name",
        ),
    ];
    for (body, field) in cases {
        let (status, response) = app.post("/api/certificates", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {response}");
        assert_eq!(fields(&response), [field]);
        assert!(!response.to_string().contains("SECRET"));
    }
    let (status, _) = app.post("/api/certificates", json!({ "name": "x" })).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a missing key is a malformed request"
    );
    assert_eq!(app.get("/api/certificates").await.1["data"], json!([]));
    assert_eq!(app.get("/api/certificates").await.1["version"], version);
}

#[tokio::test]
async fn requests_carrying_a_key_are_refused_from_a_non_loopback_peer() {
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let issued = issue(&ca, &["shop.example.com"]);
    let id = site(&app, "shop", "shop.example.com").await;

    let (status, body) = app
        .call_from(
            REMOTE,
            Method::POST,
            "/api/certificates",
            Some(upload_body("shop", &issued)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"]["code"], "FORBIDDEN");
    let selfsigned = json!({ "name": "s", "names": ["x.example.com"] });
    let (status, _) = app
        .call_from(
            REMOTE,
            Method::POST,
            "/api/certificates/self-signed",
            Some(selfsigned),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = app
        .call_from(
            REMOTE,
            Method::POST,
            &format!("/api/subsystems/{id}/certificate"),
            Some(upload_body("shop", &issued)),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        app.get("/api/certificates").await.1["data"],
        json!([]),
        "nothing was stored"
    );

    let (status, created) = app
        .post("/api/certificates", upload_body("shop", &issued))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let cert = created["data"]["id"].as_i64().unwrap();
    let uri = format!("/api/certificates/{cert}");
    let (status, _) = app
        .call_from(
            REMOTE,
            Method::PUT,
            &uri,
            Some(json!({ "keyPem": issued.key_pem })),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "replacing a key is key-bearing"
    );
    let (status, _) = app
        .call_from(
            REMOTE,
            Method::PUT,
            &uri,
            Some(json!({ "name": "renamed" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "a rename carries no key");
    let (status, _) = app
        .call_from(REMOTE, Method::GET, "/api/certificates", None)
        .await;
    assert_eq!(status, StatusCode::OK, "reads are not restricted");
    let (status, _) = app.call_from(REMOTE, Method::DELETE, &uri, None).await;
    assert_eq!(status, StatusCode::OK, "deleting carries no key");
}

#[tokio::test]
async fn self_signed_generation_creates_a_certificate() {
    let app = TestApp::with_tls(false).await;
    let (status, body) = app
        .post(
            "/api/certificates/self-signed",
            json!({ "name": "scratch", "names": ["x.example.com", "*.y.example.com"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["source"], "self_signed");
    assert_eq!(
        body["data"]["names"],
        json!(["x.example.com", "*.y.example.com"])
    );
    let (status, body) = app
        .post(
            "/api/certificates/self-signed",
            json!({ "name": "none", "names": [] }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fields(&body), ["names"]);
}

#[tokio::test]
async fn without_a_master_key_a_key_cannot_be_stored_and_the_reason_is_clear() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/api/certificates/self-signed",
            json!({ "name": "s", "names": ["x.example.com"] }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("master key"));
}

#[tokio::test]
async fn a_certificate_is_renamed_replaced_and_deleted_with_409_while_in_use() {
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let id = site(&app, "shop", "shop.example.com").await;
    let issued = issue(&ca, &["shop.example.com"]);
    let (_, created) = app
        .post("/api/certificates", upload_body("shop", &issued))
        .await;
    let cert = created["data"]["id"].as_i64().unwrap();
    let uri = format!("/api/certificates/{cert}");

    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop",
                "https": { "enabled": true, "certificateId": cert }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (status, body) = app.put(&uri, json!({ "name": "renamed" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["name"], "renamed");
    assert_eq!(body["data"]["usedBy"], json!([id]));

    let renewed = issue(&ca, &["shop.example.com"]);
    let (status, body) = app
        .put(
            &uri,
            json!({ "certPem": renewed.chain_pem, "keyPem": renewed.key_pem }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_ne!(body["data"]["fingerprint"], created["data"]["fingerprint"]);

    let (status, body) = app.delete(&uri).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _) = app.delete("/api/certificates/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .put("/api/certificates/999", json!({ "name": "x" }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    app.put(
        &format!("/api/subsystems/{id}"),
        json!({
            "name": "shop",
            "https": { "enabled": false }
        }),
    )
    .await;
    let (status, _) = app.delete(&uri).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn a_duplicate_certificate_name_is_a_conflict() {
    let app = TestApp::with_tls(true).await;
    let issued = issue(&TestCa::new("ca"), &["a.example.com"]);
    app.post("/api/certificates", upload_body("one", &issued))
        .await;
    let (status, _) = app
        .post("/api/certificates", upload_body("one", &issued))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn upload_and_attach_in_one_step_then_detach_falls_back_to_a_dev_certificate() {
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let id = site(&app, "shop", "shop.example.com").await;
    app.put(
        &format!("/api/subsystems/{id}"),
        json!({
            "name": "shop", "https": { "enabled": true }
        }),
    )
    .await;
    let dev = app.get(&format!("/api/subsystems/{id}")).await.1;
    assert_eq!(dev["data"]["https"]["certificate"]["name"], "dev-shop");

    let issued = issue(&ca, &["shop.example.com"]);
    let uri = format!("/api/subsystems/{id}/certificate");
    let (status, body) = app.post(&uri, upload_body("shop", &issued)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let detail = app.get(&format!("/api/subsystems/{id}")).await.1;
    assert_eq!(detail["data"]["https"]["certificate"]["name"], "shop");
    assert_eq!(detail["data"]["https"]["certificate"]["source"], "uploaded");
    assert_eq!(
        detail["data"]["https"]["certificate"]["uncoveredHosts"],
        json!([])
    );
    assert!(serves(&app, "shop.example.com"));

    let (status, _) = app.delete(&uri).await;
    assert_eq!(status, StatusCode::OK);
    let detail = app.get(&format!("/api/subsystems/{id}")).await.1;
    assert_eq!(detail["data"]["https"]["certificate"]["name"], "dev-shop");
    assert_eq!(detail["data"]["https"]["enabled"], true);
}

#[tokio::test]
async fn a_certificate_covering_no_host_is_rejected_and_stores_nothing() {
    let app = TestApp::with_tls(true).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let wrong = issue(&TestCa::new("ca"), &["unrelated.example.com"]);
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/certificate"),
            upload_body("wrong", &wrong),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fields(&body), ["certPem"]);
    assert_eq!(app.get("/api/certificates").await.1["data"], json!([]));

    let (_, stored) = app
        .post("/api/certificates", upload_body("wrong", &wrong))
        .await;
    let cert = stored["data"]["id"].as_i64().unwrap();
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop", "https": { "enabled": true, "certificateId": cert }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fields(&body), ["https.certificateId"]);
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop", "https": { "enabled": true, "certificateId": 999 }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fields(&body), ["https.certificateId"]);
}

#[tokio::test]
async fn a_partially_covering_certificate_lists_the_uncovered_hosts() {
    let app = TestApp::with_tls(true).await;
    let id = site(&app, "shop", "shop.example.com").await;
    app.post(
        &format!("/api/subsystems/{id}/hosts"),
        json!({ "pattern": "old.example.com" }),
    )
    .await;
    let issued = issue(&TestCa::new("ca"), &["shop.example.com"]);
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/certificate"),
            upload_body("shop", &issued),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let detail = app.get(&format!("/api/subsystems/{id}")).await.1;
    assert_eq!(
        detail["data"]["https"]["certificate"]["uncoveredHosts"],
        json!(["old.example.com"])
    );
}

#[tokio::test]
async fn https_settings_are_validated_with_nested_field_names() {
    let app = TestApp::with_tls(true).await;
    let cases = [
        (
            json!({ "enabled": false, "redirect": "308" }),
            "https.redirect",
        ),
        (
            json!({ "enabled": false, "hsts": { "enabled": true } }),
            "https.hsts.enabled",
        ),
        (
            json!({ "enabled": true, "redirect": "302" }),
            "https.redirect",
        ),
        (
            json!({ "enabled": true, "hsts": { "enabled": true, "maxAgeSecs": 99_999_999 } }),
            "https.hsts.maxAgeSecs",
        ),
    ];
    for (https, field) in cases {
        let (status, body) = app
            .post("/api/subsystems", json!({ "name": "shop", "https": https }))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {body}");
        assert_eq!(fields(&body), [field]);
    }
    let (status, body) = app
        .post(
            "/api/subsystems",
            json!({ "name": "shop", "https": { "bogus": 1 } }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

#[tokio::test]
async fn enabling_https_generates_a_dev_certificate_and_reports_effectiveness() {
    let app = TestApp::with_tls(true).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop",
                "https": {
                    "enabled": true, "redirect": "308",
                    "hsts": { "enabled": true, "maxAgeSecs": 600, "includeSubdomains": true }
                }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let https = &body["data"]["https"];
    assert_eq!(https["enabled"], true);
    assert_eq!(https["effective"], true);
    assert_eq!(https["redirect"], "308");
    assert_eq!(
        https["hsts"],
        json!({ "enabled": true, "maxAgeSecs": 600, "includeSubdomains": true })
    );
    assert_eq!(https["certificate"]["name"], "dev-shop");
    assert_eq!(https["certificate"]["source"], "self_signed");
    assert!(https["certificateId"].is_i64());
    assert!(
        serves(&app, "shop.example.com"),
        "live in the snapshot without a restart"
    );

    let (_, kept) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({ "name": "shop", "description": "x" }),
        )
        .await;
    assert_eq!(
        kept["data"]["https"]["enabled"], true,
        "omitting https keeps the settings"
    );
    assert_eq!(kept["data"]["https"]["hsts"]["maxAgeSecs"], 600);
}

#[tokio::test]
async fn with_tls_off_https_settings_are_stored_but_not_effective() {
    let app = TestApp::with_tls(false).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop", "https": { "enabled": true, "redirect": "308" }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["https"]["enabled"], true);
    assert_eq!(body["data"]["https"]["effective"], false);
    assert!(body["data"]["https"]["certificate"].is_null());
    assert_eq!(app.get("/api/certificates").await.1["data"], json!([]));
    assert!(!serves(&app, "shop.example.com"));
}

#[tokio::test]
async fn ca_bundles_are_uploaded_listed_and_deleted_with_409_while_named() {
    let app = TestApp::with_tls(false).await;
    let ca = TestCa::new("corp root");
    let (status, body) = app
        .post("/api/ca-bundles", json!({ "name": "corp", "pem": ca.pem }))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["data"]["certCount"], 1);
    assert!(body["data"]["certificates"][0]["subject"]
        .as_str()
        .unwrap()
        .contains("corp root"));
    let bundle = body["data"]["id"].as_i64().unwrap();
    assert_eq!(
        app.get("/api/ca-bundles").await.1["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let (status, body) = app
        .post(
            "/api/ca-bundles",
            json!({ "name": "bad", "pem": "no certificate SECRET" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fields(&body), ["pem"]);
    assert!(!body.to_string().contains("SECRET"));
    let (status, _) = app
        .post("/api/ca-bundles", json!({ "name": "corp", "pem": ca.pem }))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let id = site(&app, "shop", "shop.example.com").await;
    let upstream_id = {
        let (status, body) = app
            .post(
                &format!("/api/subsystems/{id}/upstreams"),
                json!({ "name": "backend", "endpoints": [{ "addr": "127.0.0.1:3000" }] }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["data"]["id"].as_i64().unwrap()
    };
    app.config
        .apply(move |conn| {
            let mut fields = fast_proxy_store::UpstreamFields {
                name: "backend".into(),
                policy: fast_proxy_model::GroupPolicy::default(),
            };
            fields.policy.tls.enabled = true;
            fields.policy.tls.ca = "corp".into();
            fast_proxy_store::repository::upstreams::update(
                conn,
                fast_proxy_model::SubsystemId(id),
                fast_proxy_model::UpstreamGroupId(upstream_id),
                &fields,
            )?;
            Ok(fast_proxy_store::Mutation {
                value: (),
                summary: "tls".into(),
            })
        })
        .await
        .unwrap();
    let (status, body) = app.delete(&format!("/api/ca-bundles/{bundle}")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body["error"]["message"].as_str().unwrap().contains("corp"));
    let (status, _) = app.delete("/api/ca-bundles/999").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_unused_ca_bundle_is_deleted() {
    let app = TestApp::with_tls(false).await;
    let (_, body) = app
        .post(
            "/api/ca-bundles",
            json!({ "name": "corp", "pem": TestCa::new("root").pem }),
        )
        .await;
    let id = body["data"]["id"].as_i64().unwrap();
    let (status, _) = app.delete(&format!("/api/ca-bundles/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.get("/api/ca-bundles").await.1["data"], json!([]));
}

#[tokio::test]
async fn the_tls_endpoint_reports_the_switch_and_the_vault() {
    let on = TestApp::with_tls(true).await;
    let (status, body) = on.get("/api/tls").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["enabled"], true);
    assert_eq!(body["data"]["listen"], "127.0.0.1:8443");
    assert_eq!(body["data"]["minVersion"], "1.2");
    assert_eq!(body["data"]["vault"], "ok");
    let off = TestApp::new().await;
    let body = off.get("/api/tls").await.1;
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["listen"], Value::Null);
    assert_eq!(body["data"]["vault"], "not created");
}

#[tokio::test]
async fn certificate_events_carry_names_never_key_material() {
    let app = TestApp::with_tls(true).await;
    let mut events = app.state.events.subscribe();
    let issued = issue(&TestCa::new("ca"), &["a.example.com"]);
    app.post("/api/certificates", upload_body("shop", &issued))
        .await;
    app.post(
        "/api/certificates/self-signed",
        json!({ "name": "s", "names": ["x.example.com"] }),
    )
    .await;

    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(serde_json::to_value(&event).unwrap());
    }
    let cert_events: Vec<&Value> = seen.iter().filter(|e| e["type"] == "certState").collect();
    assert_eq!(cert_events.len(), 2);
    assert_eq!(cert_events[0]["change"], "uploaded");
    assert_eq!(cert_events[0]["certificate"], "shop");
    assert_eq!(cert_events[1]["change"], "generated");
    let everything = serde_json::to_string(&seen).unwrap();
    assert!(!everything.contains(&key_body(&issued)) && !everything.contains("PRIVATE"));
    let metrics = app.metrics.render();
    assert!(!metrics.contains(&key_body(&issued)) && !metrics.contains("PRIVATE"));
    assert!(metrics.contains("fastproxy_tls_cert_expiry_seconds{cert=\"shop\""));
}

#[tokio::test]
async fn an_oversized_body_is_refused() {
    let app = TestApp::with_tls(true).await;
    let huge = "x".repeat(700 * 1024);
    let (status, _) = app
        .call_raw(
            Method::POST,
            "/api/certificates",
            Some(&json!({ "name": "x", "certPem": huge, "keyPem": "y" }).to_string()),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

async fn route_to_backend(app: &TestApp, id: i64) -> i64 {
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({ "name": "backend", "endpoints": [{ "addr": "127.0.0.1:3000" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let upstream = body["data"]["id"].as_i64().unwrap();
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/routes"),
            json!({ "prefix": "/", "upstreamGroupId": upstream }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    upstream
}

#[tokio::test]
async fn the_resolver_shows_the_redirect_https_service_and_upstream_scheme() {
    let app = TestApp::with_tls(true).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let upstream = route_to_backend(&app, id).await;
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({
                "name": "shop",
                "https": { "enabled": true, "redirect": "308" }
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let (_, resolved) = app
        .post(
            "/api/resolve",
            json!({ "host": "shop.example.com", "path": "/cart?x=1" }),
        )
        .await;
    let https = &resolved["data"]["https"];
    assert_eq!(https["servedOverHttps"], true);
    assert_eq!(https["plainHttpRedirect"]["status"], 308);
    assert_eq!(
        https["plainHttpRedirect"]["location"],
        "https://shop.example.com:8443/cart?x=1"
    );
    assert_eq!(resolved["data"]["route"]["upstreamScheme"], "http");

    app.config
        .apply(move |conn| {
            let mut fields = fast_proxy_store::UpstreamFields {
                name: "backend".into(),
                policy: fast_proxy_model::GroupPolicy::default(),
            };
            fields.policy.tls.enabled = true;
            fast_proxy_store::repository::upstreams::update(
                conn,
                fast_proxy_model::SubsystemId(id),
                fast_proxy_model::UpstreamGroupId(upstream),
                &fields,
            )?;
            Ok(fast_proxy_store::Mutation {
                value: (),
                summary: "tls".into(),
            })
        })
        .await
        .unwrap();
    let (_, resolved) = app
        .post(
            "/api/resolve",
            json!({ "host": "shop.example.com", "path": "/x" }),
        )
        .await;
    assert_eq!(resolved["data"]["route"]["upstreamScheme"], "https");
    assert_eq!(
        resolved["data"]["route"]["endpoints"][0]["upstreamUrl"],
        "https://127.0.0.1:3000/x"
    );

    let (_, unknown) = app
        .post(
            "/api/resolve",
            json!({ "host": "nobody.example.com", "path": "/" }),
        )
        .await;
    assert!(unknown["data"]["https"].is_null());
}

#[tokio::test]
async fn the_resolver_reports_no_redirect_when_tls_is_off_or_the_mode_is_off() {
    let off = TestApp::with_tls(false).await;
    let id = site(&off, "shop", "shop.example.com").await;
    route_to_backend(&off, id).await;
    off.put(
        &format!("/api/subsystems/{id}"),
        json!({
            "name": "shop", "https": { "enabled": true, "redirect": "308" }
        }),
    )
    .await;
    let (_, resolved) = off
        .post(
            "/api/resolve",
            json!({ "host": "shop.example.com", "path": "/" }),
        )
        .await;
    assert_eq!(resolved["data"]["https"]["servedOverHttps"], false);
    assert!(resolved["data"]["https"]["plainHttpRedirect"].is_null());

    let on = TestApp::with_tls(true).await;
    let id = site(&on, "shop", "shop.example.com").await;
    route_to_backend(&on, id).await;
    on.put(
        &format!("/api/subsystems/{id}"),
        json!({
            "name": "shop", "https": { "enabled": true }
        }),
    )
    .await;
    let (_, resolved) = on
        .post(
            "/api/resolve",
            json!({ "host": "shop.example.com", "path": "/" }),
        )
        .await;
    assert_eq!(resolved["data"]["https"]["servedOverHttps"], true);
    assert!(resolved["data"]["https"]["plainHttpRedirect"].is_null());
}

#[tokio::test]
async fn the_status_reports_the_tls_state_and_the_soonest_certificate_expiry() {
    let app = TestApp::with_tls(true).await;
    let (_, empty) = app.get("/api/status").await;
    assert_eq!(empty["data"]["tls"]["enabled"], true);
    assert_eq!(empty["data"]["tls"]["listen"], "127.0.0.1:8443");
    assert_eq!(
        empty["data"]["tls"]["vault"], "ok",
        "a key is loaded; nothing is sealed yet"
    );
    assert_eq!(empty["data"]["tls"]["certificates"], 0);
    assert!(empty["data"]["tls"]["soonestExpiry"].is_null());

    let ca = TestCa::new("ca");
    let later = ca.issue(&LeafSpec::new(&["a.example.com"]).expiring_in_days(200));
    let sooner = ca.issue(&LeafSpec::new(&["b.example.com"]).expiring_in_days(20));
    app.post("/api/certificates", upload_body("later", &later))
        .await;
    app.post("/api/certificates", upload_body("sooner", &sooner))
        .await;
    let (_, status) = app.get("/api/status").await;
    let tls = &status["data"]["tls"];
    assert_eq!(tls["certificates"], 2);
    assert_eq!(tls["unreadable"], 0);
    assert_eq!(tls["vault"], "ok");
    assert_eq!(tls["soonestExpiry"]["name"], "sooner");
    let left = tls["soonestExpiry"]["secondsLeft"].as_i64().unwrap();
    assert!(left > 19 * 86_400 && left <= 20 * 86_400, "{left}");

    let off = TestApp::new().await;
    assert_eq!(
        off.get("/api/status").await.1["data"]["tls"]["enabled"],
        false
    );
}

#[tokio::test]
async fn an_upstream_group_carries_its_tls_settings_with_defaults_and_validation() {
    let app = TestApp::with_tls(false).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let ca = TestCa::new("corp root");
    app.post("/api/ca-bundles", json!({ "name": "corp", "pem": ca.pem }))
        .await;

    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({ "name": "plain", "endpoints": [{ "addr": "127.0.0.1:3000" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["data"]["tls"],
        json!({ "enabled": false, "verify": true, "serverName": "", "ca": "" }),
        "off by default, verifying, OS trust store"
    );

    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({
                "name": "secure",
                "tls": { "enabled": true, "verify": false, "serverName": "backend.internal", "ca": "corp" },
                "endpoints": [{ "addr": "127.0.0.1:3001" }]
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(
        body["data"]["tls"],
        json!({ "enabled": true, "verify": false, "serverName": "backend.internal", "ca": "corp" })
    );
    let group = body["data"]["id"].as_i64().unwrap();

    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}/upstreams/{group}"),
            json!({ "name": "secure", "tls": { "enabled": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["tls"]["verify"], true);
    assert_eq!(body["data"]["tls"]["ca"], "");
}

#[tokio::test]
async fn an_unknown_ca_name_or_a_bad_server_name_is_a_400_naming_the_field() {
    let app = TestApp::with_tls(false).await;
    let id = site(&app, "shop", "shop.example.com").await;
    let uri = format!("/api/subsystems/{id}/upstreams");
    let endpoints = json!([{ "addr": "127.0.0.1:3000" }]);

    let (status, body) = app
        .post(&uri, json!({ "name": "g", "tls": { "enabled": true, "ca": "missing" }, "endpoints": endpoints }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fields(&body), ["tls.ca"]);

    let (status, body) = app
        .post(&uri, json!({ "name": "g", "tls": { "enabled": true, "serverName": "bad name" }, "endpoints": endpoints }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(fields(&body), ["tls.serverName"]);

    let (status, body) = app
        .post(
            &uri,
            json!({ "name": "g", "tls": { "enabled": true, "bogus": 1 }, "endpoints": endpoints }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        app.get(&uri).await.1["data"],
        json!([]),
        "nothing was created"
    );
}
