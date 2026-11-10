//! Every write made through the API lands in the audit log, attributed and
//! with the stable action name of the handler that made it.

use axum::http::{Method, StatusCode};
use fast_proxy_store::repository::audit::{self, AuditEntry, AuditQuery};
use serde_json::json;

use crate::test_support::TestApp;

async fn audit_entries(app: &TestApp) -> Vec<AuditEntry> {
    app.config
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 100))
        .await
        .unwrap()
        .value
}

#[tokio::test]
async fn an_api_write_is_audited_with_the_peer_and_its_action() {
    let app = TestApp::new().await;
    let (status, body) = app
        .call_from(
            "192.0.2.9:5555",
            Method::POST,
            "/api/subsystems",
            Some(json!({ "name": "shop" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let entries = audit_entries(&app).await;
    assert_eq!(entries.len(), 1);
    let entry = &entries[0].entry;
    assert_eq!(entry.action, "subsystem.create");
    assert_eq!(entry.target, "shop");
    assert_eq!(entry.source, "192.0.2.9");
    assert_eq!(entry.actor, "test-admin", "named by the token");
    assert_eq!(entry.role, "admin");
    assert_eq!(entry.revision, body["version"].as_u64());
}

#[tokio::test]
async fn a_rejected_request_leaves_no_audit_entry() {
    let app = TestApp::new().await;
    let (status, _) = app
        .post("/api/subsystems", json!({ "name": "Not Valid" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(audit_entries(&app).await.is_empty());
}

#[tokio::test]
async fn each_kind_of_write_names_its_own_action() {
    let app = TestApp::new().await;
    let (_, body) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    let id = body["data"]["id"].as_i64().unwrap();
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/hosts"),
            json!({ "pattern": "shop.example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = app
        .call(Method::DELETE, &format!("/api/subsystems/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);

    let actions: Vec<String> = audit_entries(&app)
        .await
        .into_iter()
        .map(|row| row.entry.action)
        .collect();
    assert_eq!(
        actions,
        ["subsystem.delete", "host.add", "subsystem.create"]
    );
}

#[tokio::test]
async fn with_authentication_off_a_write_is_recorded_as_anonymous() {
    let mut app = TestApp::with_auth_off().await;
    app.sign_out();
    let (status, _) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::CREATED);
    let entries = audit_entries(&app).await;
    assert_eq!(entries[0].entry.actor, "anonymous");
}
