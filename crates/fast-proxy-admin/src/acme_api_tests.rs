//! The ACME endpoints (M4, A1/A2): the process view, a subsystem's status and
//! preview, a forced renewal with its refusals, roles, and the live event.

use axum::http::{Method, StatusCode};
use fast_proxy_model::{AcmeState, AcmeStatus, Role, SubsystemId};
use serde_json::json;

use crate::events::Event;
use crate::test_support::TestApp;

/// A subsystem with a host and its ACME switch on; returns its id.
async fn acme_site(app: &TestApp) -> i64 {
    let (id, _) = app.site("shop", "shop.example.com").await;
    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({ "name": "shop", "https": { "enabled": true, "acmeEnabled": true } }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    id
}

async fn record(app: &TestApp, id: i64, change: impl FnOnce(&mut AcmeStatus)) {
    let mut status = AcmeStatus::new(SubsystemId(id));
    change(&mut status);
    app.config.record_acme_status(status).await.unwrap();
}

fn now_ms() -> i64 {
    crate::clock::now_ms()
}

#[tokio::test]
async fn with_acme_off_the_overview_says_so_and_a_renewal_is_a_conflict() {
    let app = TestApp::with_tls(true).await;
    let (status, body) = app.get("/api/acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["data"]["directory"], json!(null));
    let (id, _) = app.site("shop", "shop.example.com").await;
    let (status, body) = app.get(&format!("/api/subsystems/{id}/acme")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["available"], false);
    assert_eq!(body["data"]["preview"], json!(null));
    let (status, body) = app
        .post(&format!("/api/subsystems/{id}/acme/renew"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
}

#[tokio::test]
async fn the_overview_shows_the_settings_the_lease_and_every_status() {
    let app = TestApp::with_acme().await;
    let id = acme_site(&app).await;
    record(&app, id, |s| {
        s.state = AcmeState::Failed;
        s.eligible_names = vec!["shop.example.com".into()];
        s.last_error = "dns: NXDOMAIN".into();
        s.failures = 1;
    })
    .await;
    assert!(app
        .config
        .try_acme_lease(std::time::Duration::from_secs(60))
        .await
        .unwrap());

    let (status, body) = app.get("/api/acme").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["enabled"], true);
    assert_eq!(data["staging"], true);
    assert_eq!(data["directory"], "https://acme-staging.example/directory");
    assert_eq!(data["allowedDomains"], json!(["*.example.com"]));
    assert_eq!(data["accountRegistered"], false);
    assert_eq!(data["lease"]["heldHere"], true);
    let shop = &data["subsystems"][0];
    assert_eq!(shop["subsystem"], "shop");
    assert_eq!(shop["state"], "failed");
    assert_eq!(shop["lastError"], "dns: NXDOMAIN");
    assert_eq!(shop["eligibleNames"], json!(["shop.example.com"]));
}

#[tokio::test]
async fn a_subsystem_view_previews_what_an_order_would_cover() {
    let app = TestApp::with_acme().await;
    let id = acme_site(&app).await;
    for host in ["*.wild.example.com", "shop.other.net"] {
        let (status, body) = app
            .post(
                &format!("/api/subsystems/{id}/hosts"),
                json!({ "pattern": host }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let (status, body) = app.get(&format!("/api/subsystems/{id}/acme")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["available"], true);
    assert_eq!(data["acmeEnabled"], true);
    assert_eq!(data["status"], json!(null), "nothing ordered yet");
    assert_eq!(
        data["preview"]["eligibleNames"],
        json!(["shop.example.com"])
    );
    assert_eq!(
        data["preview"]["skippedNames"],
        json!([
            { "name": "*.wild.example.com", "reason": "wildcard_needs_dns01" },
            { "name": "shop.other.net", "reason": "not_allowed" }
        ])
    );
    let (status, _) = app.get("/api/subsystems/999/acme").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_renewal_is_accepted_and_audited() {
    let app = TestApp::with_acme().await;
    let id = acme_site(&app).await;
    let (status, body) = app
        .post(&format!("/api/subsystems/{id}/acme/renew"), json!({}))
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let (_, audit) = app.get("/api/audit?action=acme.renew.request").await;
    assert_eq!(
        audit["data"][0]["target"],
        format!("subsystem:{id}"),
        "{audit}"
    );
}

#[tokio::test]
async fn a_renewal_is_refused_while_an_order_runs_when_the_cap_is_spent_or_acme_is_off() {
    let app = TestApp::with_acme().await;
    let id = acme_site(&app).await;
    let renew = format!("/api/subsystems/{id}/acme/renew");

    record(&app, id, |s| {
        s.state = AcmeState::Renewing;
        s.last_attempt_at_ms = Some(now_ms());
    })
    .await;
    assert_eq!(app.post(&renew, json!({})).await.0, StatusCode::CONFLICT);

    record(&app, id, |s| {
        s.state = AcmeState::Failed;
        s.window_failures = 5;
        s.failure_window_start_ms = Some(now_ms());
    })
    .await;
    let response = app
        .send(
            app.request_builder(Method::POST, &renew)
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((3000..=3601).contains(&retry), "{retry}");

    let (id_off, _) = app.site("blog", "blog.example.com").await;
    let (status, _) = app
        .post(&format!("/api/subsystems/{id_off}/acme/renew"), json!({}))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_viewer_reads_the_status_but_cannot_renew() {
    let mut app = TestApp::with_acme().await;
    let id = acme_site(&app).await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    assert_eq!(app.get("/api/acme").await.0, StatusCode::OK);
    assert_eq!(
        app.get(&format!("/api/subsystems/{id}/acme")).await.0,
        StatusCode::OK
    );
    let (status, _) = app
        .post(&format!("/api/subsystems/{id}/acme/renew"), json!({}))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_status_written_here_reaches_the_websocket() {
    let app = TestApp::with_acme().await;
    crate::spawn_background_tasks(app.state.clone());
    let id = acme_site(&app).await;
    let mut events = app.state.events.subscribe();
    record(&app, id, |s| {
        s.state = AcmeState::Pending;
        s.last_error = String::new();
    })
    .await;
    loop {
        if let Event::AcmeStatus {
            subsystem_id,
            state,
            ..
        } = TestApp::next_event(&mut events).await
        {
            assert_eq!((subsystem_id, state), (id, "pending"));
            break;
        }
    }
}
