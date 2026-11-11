//! The audit log over the API and what the middleware records: paging, filters
//! and the export; authentication failures, refusals, foreign hosts and rejected
//! writes (and not committed changes twice); and that no secret ever lands in it.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use fast_proxy_model::Role;
use http_body_util::BodyExt;
use serde_json::{json, Value};

use crate::test_support::TestApp;

async fn entries(app: &TestApp, query: &str) -> Vec<Value> {
    let (status, body) = app.get(&format!("/api/audit{query}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].as_array().unwrap().clone()
}

fn actions(entries: &[Value]) -> Vec<&str> {
    entries
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect()
}

async fn make_subsystems(app: &TestApp, count: usize) {
    for n in 0..count {
        let (status, _) = app
            .post("/api/subsystems", json!({ "name": format!("site-{n}") }))
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
}

// ---- the list ----

#[tokio::test]
async fn the_list_is_newest_first_and_each_entry_is_complete() {
    let app = TestApp::new().await;
    make_subsystems(&app, 2).await;
    let list = entries(&app, "").await;
    assert_eq!(actions(&list), ["subsystem.create", "subsystem.create"]);
    let newest = &list[0];
    assert_eq!(newest["target"], "site-1");
    assert_eq!(newest["actor"], "test-admin");
    assert_eq!(newest["role"], "admin");
    assert_eq!(newest["outcome"], "ok");
    assert_eq!(newest["source"], "127.0.0.1");
    assert!(newest["requestId"].as_str().unwrap().contains('-'));
    assert!(newest["atMs"].as_i64().unwrap() > 0);
    assert_eq!(newest["revision"], 2);
    assert!(
        newest["detail"].is_object(),
        "the detail is structured JSON"
    );
    assert!(newest["id"].as_i64().unwrap() > list[1]["id"].as_i64().unwrap());
}

#[tokio::test]
async fn paging_walks_the_whole_log_without_repeats_or_gaps() {
    let app = TestApp::new().await;
    make_subsystems(&app, 5).await;
    let mut seen = Vec::new();
    let mut uri = "/api/audit?limit=2".to_string();
    loop {
        let (_, page) = app.get(&uri).await;
        for entry in page["data"].as_array().unwrap() {
            seen.push(entry["id"].as_i64().unwrap());
        }
        match page["nextBefore"].as_i64() {
            Some(before) => uri = format!("/api/audit?limit=2&before={before}"),
            None => break,
        }
    }
    assert_eq!(seen.len(), 5);
    let mut sorted = seen.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(seen, sorted, "newest first, no repeats");
}

#[tokio::test]
async fn the_last_page_has_no_next_page_and_the_limit_is_capped() {
    let app = TestApp::new().await;
    make_subsystems(&app, 3).await;
    let (_, page) = app.get("/api/audit?limit=10").await;
    assert!(page["nextBefore"].is_null());
    let (_, page) = app.get("/api/audit?limit=100000").await;
    assert_eq!(page["data"].as_array().unwrap().len(), 3);
    let (_, page) = app.get("/api/audit?limit=0").await;
    assert_eq!(page["data"].as_array().unwrap().len(), 1, "at least one");
}

#[tokio::test]
async fn each_filter_narrows_the_list() {
    let mut app = TestApp::new().await;
    let admin = app.token.clone().unwrap();
    make_subsystems(&app, 1).await;
    app.post("/api/tokens", json!({ "name": "viewer", "role": "viewer" }))
        .await;
    let (_, created) = app
        .post("/api/tokens", json!({ "name": "eyes", "role": "viewer" }))
        .await;
    let secret = created["data"]["secret"].as_str().unwrap().to_string();
    app.sign_in_with(secret);
    app.post("/api/subsystems", json!({ "name": "blocked" }))
        .await; // 403, a denial
    app.sign_in_with(admin);

    let by_action = entries(&app, "?action=token.create").await;
    assert_eq!(by_action.len(), 2);
    let by_outcome = entries(&app, "?outcome=denied").await;
    assert_eq!(actions(&by_outcome), ["access.denied"]);
    let by_actor = entries(&app, "?actor=eyes").await;
    assert!(by_actor.iter().all(|e| e["actor"] == "eyes"));
    assert!(!by_actor.is_empty());
    let future = crate::clock::now_ms() + 60_000;
    assert!(entries(&app, &format!("?since={future}")).await.is_empty());
    assert!(!entries(&app, "?since=0").await.is_empty());
}

#[tokio::test]
async fn bad_parameters_are_refused_by_field() {
    let app = TestApp::new().await;
    for (query, field) in [
        ("?limit=abc", "limit"),
        ("?before=-3", "before"),
        ("?since=yesterday", "since"),
        ("?outcome=maybe", "outcome"),
    ] {
        let (status, body) = app.get(&format!("/api/audit{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        assert_eq!(body["error"]["fields"][0]["field"], field);
    }
}

#[tokio::test]
async fn a_viewer_may_read_the_list_but_not_export() {
    let mut app = TestApp::new().await;
    make_subsystems(&app, 1).await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    assert_eq!(app.get("/api/audit").await.0, StatusCode::OK);
    assert_eq!(app.get("/api/audit/export").await.0, StatusCode::FORBIDDEN);
}

// ---- the export ----

async fn export(app: &TestApp, query: &str) -> (axum::http::HeaderMap, String) {
    let request = app
        .request_builder(Method::GET, &format!("/api/audit/export{query}"))
        .body(Body::empty())
        .unwrap();
    let response = app.send(request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (headers, String::from_utf8(bytes.to_vec()).unwrap())
}

#[tokio::test]
async fn the_export_is_ndjson_oldest_first_and_complete() {
    let app = TestApp::new().await;
    make_subsystems(&app, 3).await;
    let (headers, text) = export(&app, "").await;
    assert_eq!(headers["content-type"], "application/x-ndjson");
    assert!(headers["content-disposition"]
        .to_str()
        .unwrap()
        .contains("audit.ndjson"));
    let lines: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    let ids: Vec<i64> = lines.iter().map(|l| l["id"].as_i64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "oldest first");
    assert_eq!(lines[0]["target"], "site-0");
}

#[tokio::test]
async fn a_large_log_streams_in_several_chunks_and_loses_nothing() {
    let app = TestApp::new().await;
    app.config
        .read(|conn| {
            for n in 0..1_250_i64 {
                fast_proxy_store::repository::audit::append(
                    conn,
                    &fast_proxy_store::repository::audit::NewAuditEntry {
                        at_ms: n,
                        instance: "i".into(),
                        actor: "bulk".into(),
                        role: "admin".into(),
                        source: "s".into(),
                        request_id: "r".into(),
                        action: "x.y".into(),
                        target: format!("t{n}"),
                        outcome: fast_proxy_store::repository::audit::AuditOutcome::Ok,
                        detail: "{}".into(),
                        revision: None,
                    },
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let (_, text) = export(&app, "").await;
    assert_eq!(text.lines().count(), 1_250);
    let (_, recent) = export(&app, "?since=1200").await;
    assert_eq!(recent.lines().count(), 50);
}

// ---- what the middleware records ----

#[tokio::test]
async fn a_wrong_token_is_recorded_with_its_reason_and_never_the_token() {
    let mut app = TestApp::new().await;
    let admin = app.token.clone().unwrap();
    app.sign_in_with("fp_99.this-secret-must-not-be-logged".into());
    assert_eq!(app.get("/api/status").await.0, StatusCode::UNAUTHORIZED);
    app.sign_out();
    assert_eq!(app.get("/api/status").await.0, StatusCode::UNAUTHORIZED);
    app.sign_in_with(admin);

    let failures = entries(&app, "?action=auth.failed").await;
    assert_eq!(failures.len(), 2);
    assert_eq!(failures[0]["actor"], "anonymous");
    assert_eq!(failures[0]["outcome"], "denied");
    assert_eq!(failures[0]["target"], "/api/status");
    assert_eq!(failures[0]["detail"]["reason"], "no token presented");
    assert_eq!(failures[1]["detail"]["reason"], "unknown token");
    assert_eq!(failures[1]["detail"]["status"], 401);
    assert!(!app
        .get("/api/audit")
        .await
        .1
        .to_string()
        .contains("this-secret-must-not"));
}

#[tokio::test]
async fn the_failure_that_trips_the_throttle_says_so_and_later_refusals_are_not_logged() {
    let mut app = TestApp::new().await;
    let admin = app.token.clone().unwrap();
    app.sign_out();
    let peer = "198.51.100.7:1";
    for _ in 0..14 {
        app.call_from(peer, Method::GET, "/api/status", None).await;
    }
    app.sign_in_with(admin);
    let failures = entries(&app, "?action=auth.failed").await;
    let throttled: Vec<bool> = failures
        .iter()
        .map(|e| e["detail"]["throttled"].as_bool().unwrap())
        .collect();
    assert_eq!(
        failures.len(),
        10,
        "ten failures were recorded, then the address was throttled"
    );
    assert_eq!(throttled.iter().filter(|t| **t).count(), 1);
    assert!(
        throttled[0],
        "the newest of the ten is the one that tripped it"
    );
}

#[tokio::test]
async fn a_forbidden_request_names_the_caller() {
    let mut app = TestApp::new().await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    assert_eq!(
        app.post("/api/subsystems", json!({ "name": "nope" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    app.sign_in_with(app.token("auditor", Role::Admin).await);
    let denied = entries(&app, "?action=access.denied").await;
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0]["actor"], "eyes");
    assert_eq!(denied[0]["role"], "viewer");
    assert_eq!(denied[0]["detail"]["method"], "POST");
}

#[tokio::test]
async fn a_foreign_host_is_recorded_and_the_value_is_bounded() {
    let app = TestApp::new().await;
    let long = format!("{}.example", "h".repeat(400));
    for host in ["evil.example", long.as_str()] {
        let request = Request::builder()
            .uri("/api/status")
            .header("host", host)
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.send(request).await.status(),
            StatusCode::MISDIRECTED_REQUEST
        );
    }
    let refused = entries(&app, "?action=host.refused").await;
    assert_eq!(refused.len(), 2);
    assert_eq!(refused[1]["detail"]["host"], "evil.example");
    assert_eq!(refused[0]["detail"]["host"].as_str().unwrap().len(), 200);
}

#[tokio::test]
async fn a_rejected_write_is_recorded_once_and_a_committed_one_only_by_its_own_entry() {
    let app = TestApp::new().await;
    let (status, _) = app
        .post("/api/subsystems", json!({ "name": "Not Valid" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::CONFLICT);

    let all = entries(&app, "").await;
    assert_eq!(
        actions(&all),
        ["request.rejected", "subsystem.create", "request.rejected"]
    );
    assert_eq!(all[0]["outcome"], "failed");
    assert_eq!(all[0]["detail"]["status"], 409);
    assert_eq!(all[0]["actor"], "test-admin");
    assert_eq!(all[1]["outcome"], "ok");
    assert_eq!(all[2]["detail"]["status"], 400);
    assert!(all[0]["revision"].is_null());
}

#[tokio::test]
async fn reads_and_not_founds_are_not_recorded() {
    let app = TestApp::new().await;
    assert_eq!(app.get("/api/subsystems/99").await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.get("/api/status").await.0, StatusCode::OK);
    assert!(entries(&app, "").await.is_empty());
}

#[tokio::test]
async fn the_websocket_ticket_never_reaches_the_log() {
    let app = TestApp::new().await;
    let request = app
        .request_builder(Method::GET, "/ws?ticket=THE-TICKET-VALUE")
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.send(request).await.status(), StatusCode::UNAUTHORIZED);
    let text = app.get("/api/audit").await.1.to_string();
    assert!(text.contains("\"/ws\""), "the path is logged: {text}");
    assert!(!text.contains("THE-TICKET-VALUE"));
}

#[tokio::test]
async fn a_flood_of_unauthenticated_requests_cannot_grow_the_log_without_bound() {
    let mut app = TestApp::new().await;
    let admin = app.token.clone().unwrap();
    app.sign_out();
    for n in 0..150u32 {
        let peer = format!("10.9.{}.{}:1", n / 200, n % 200);
        app.call_from(&peer, Method::GET, "/api/status", None).await;
    }
    // Each address failed once (below the throttle); the process-wide bound applies.
    app.sign_in_with(admin);
    let recorded = entries(&app, "?action=auth.failed&limit=200").await;
    assert!(recorded.len() <= 60, "{} entries", recorded.len());
    assert!(recorded.len() >= 50, "{} entries", recorded.len());
}

// ---- tokens ----

#[tokio::test]
async fn a_tokens_first_use_is_recorded_once() {
    let mut app = TestApp::new().await;
    let (_, created) = app
        .post("/api/tokens", json!({ "name": "ci", "role": "viewer" }))
        .await;
    let secret = created["data"]["secret"].as_str().unwrap().to_string();
    let admin = app.token.clone().unwrap();
    app.sign_in_with(secret);
    app.get("/api/status").await;
    app.get("/api/status").await;
    app.sign_in_with(admin);

    let first = entries(&app, "?action=token.first_use").await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0]["actor"], "ci");
    let (_, tokens) = app.get("/api/tokens").await;
    let ci = tokens["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "ci")
        .unwrap();
    assert!(
        ci["lastUsedAtMs"].is_i64(),
        "the list shows when it was last used"
    );
}

// ---- hygiene ----

#[tokio::test]
async fn no_secret_key_or_pem_ever_lands_in_the_log() {
    use fast_proxy_test_pki::{LeafSpec, TestCa};
    let app = TestApp::with_tls(true).await;
    let ca = TestCa::new("ca");
    let issued = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let (status, _) = app
        .post(
            "/api/certificates",
            json!({ "name": "shop", "certPem": issued.chain_pem, "keyPem": issued.key_pem }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, created) = app
        .post("/api/tokens", json!({ "name": "ci", "role": "viewer" }))
        .await;
    let secret = created["data"]["secret"].as_str().unwrap().to_string();
    let bare = secret.rsplit('.').next().unwrap().to_string();
    app.post("/api/maintenance/global", json!({})).await;
    app.call(
        Method::PUT,
        "/api/maintenance/global",
        Some(json!({ "bypass": { "header": "X-Pass", "secret": "super-secret-bypass-value" } })),
    )
    .await;

    let (_, text) = export(&app, "").await;
    let key_body: String = issued
        .key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    for needle in [
        "BEGIN",
        "PRIVATE KEY",
        key_body.as_str(),
        bare.as_str(),
        "super-secret-bypass-value",
    ] {
        assert!(
            !text.contains(needle),
            "{needle:?} leaked into the audit log"
        );
    }
    assert!(text.contains("certificate.upload"));
    assert!(text.contains("token.create"));
}
