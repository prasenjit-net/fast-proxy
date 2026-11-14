//! The sampling settings API: reading, replacing, the overrides, validation, the
//! audit trail, the events, and that a change is live in the snapshot.

use axum::http::{Method, StatusCode};
use fast_proxy_model::Role;
use fast_proxy_store::repository::audit::{self, AuditQuery};
use serde_json::{json, Value};

use crate::events::Event;
use crate::test_support::TestApp;

/// The body of a `PUT /api/sampling`: the defaults with `changes` applied.
fn settings(changes: Value) -> Value {
    let mut body = json!({
        "enabled": true,
        "triggers": ["upstream_failure", "retried", "slow", "client_errors",
                     "oversized_request", "oversized_response", "unusual_headers", "baseline"],
        "slowThresholdMs": 2000,
        "slowP99FactorX10": 30,
        "slowP99FloorMs": 50,
        "clientErrorsPerMin": 100,
        "oversizedRequestBytes": 8_388_608,
        "oversizedResponseBytes": 33_554_432,
        "baselinePerMille": 1,
        "ratePerSec": 20,
        "rateBurst": 40,
        "stormBurst": 3,
        "stormIntervalSecs": 30,
        "retentionDays": 7,
        "maxRows": 100_000,
        "maxBytes": 268_435_456,
        "bodyRequestCap": 4096,
        "bodyResponseCap": 4096,
        "extraHeaders": [],
        "redactHeaders": [],
        "redactQueryParams": [],
        "redactBodyFields": [],
    });
    for (name, value) in changes.as_object().unwrap() {
        body[name] = value.clone();
    }
    body
}

async fn audit_entries(app: &TestApp, action: &str) -> Vec<audit::AuditEntry> {
    let action = action.to_string();
    app.config
        .read(move |conn| {
            audit::list(
                conn,
                &AuditQuery {
                    action: Some(action),
                    ..Default::default()
                },
                None,
                10,
            )
        })
        .await
        .unwrap()
        .value
}

#[tokio::test]
async fn before_anything_is_written_the_defaults_are_shown_with_the_status() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/sampling").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let settings = &body["data"]["settings"];
    assert_eq!(settings["enabled"], true);
    assert!(settings["triggers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|t| t != "slow_p99"));
    assert_eq!(settings["baselinePerMille"], 1);
    assert_eq!(settings["retentionDays"], 7);
    assert_eq!(settings["stormBurst"], 3);
    let built_in = &settings["builtInRedaction"];
    assert!(built_in["headers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h == "authorization"));
    assert!(built_in["queryParams"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h == "token"));
    assert!(built_in["bodyFields"]
        .as_array()
        .unwrap()
        .iter()
        .any(|h| h == "password"));
    assert_eq!(body["data"]["overrides"], json!([]));
    let status = &body["data"]["status"];
    assert_eq!(status["processEnabled"], true);
    assert_eq!(
        (status["captured"].as_u64(), status["dropped"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(status["table"]["rows"], 0);
}

#[tokio::test]
async fn the_status_counts_the_stored_samples_and_this_processs_totals() {
    let app = TestApp::new().await;
    app.store_samples(vec![
        crate::samples_api_tests_support::sample(1),
        crate::samples_api_tests_support::sample(2),
    ]);
    app.metrics.sample_captured("slow");
    app.metrics
        .sample_dropped(fast_proxy_telemetry::SampleDropReason::RateLimit);
    let (_, body) = app.get("/api/sampling").await;
    let status = &body["data"]["status"];
    assert_eq!(status["table"]["rows"], 2);
    assert!(status["table"]["bytes"].as_u64().unwrap() > 0);
    assert_eq!(status["table"]["oldestAtMs"], 1);
    assert_eq!(
        (status["captured"].as_u64(), status["dropped"].as_u64()),
        (Some(1), Some(1))
    );
}

#[tokio::test]
async fn replacing_the_settings_is_a_revision_with_the_actor_an_audit_entry_and_an_event() {
    let app = TestApp::new().await;
    let before = app.state.config.snapshot_holder().load().version;
    let mut events = app.state.events.subscribe();
    let body = settings(json!({
        "retentionDays": 3,
        "baselinePerMille": 25,
        "triggers": ["slow_p99", "baseline"],
        "redactQueryParams": ["sessionid"],
        "extraHeaders": ["x-tenant"],
    }));
    let (status, put) = app.put("/api/sampling", body).await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(put["version"], before + 1);
    assert_eq!(put["data"]["retentionDays"], 3);
    assert_eq!(put["data"]["updatedBy"], "test-admin");
    assert_eq!(put["data"]["triggers"], json!(["slow_p99", "baseline"]));

    let (_, read) = app.get("/api/sampling").await;
    assert_eq!(
        read["data"]["settings"]["redactQueryParams"],
        json!(["sessionid"])
    );
    assert_eq!(
        read["data"]["settings"]["extraHeaders"],
        json!(["x-tenant"])
    );
    let live = app.state.config.snapshot_holder().load();
    assert_eq!(live.sampling_settings.retention_days, 3);
    assert_eq!(live.sampling_settings.baseline_per_mille, 25);

    let entry = &audit_entries(&app, "sampling.update").await[0].entry;
    assert_eq!(
        (entry.actor.as_str(), entry.target.as_str()),
        ("test-admin", "global")
    );
    let Event::SamplingChanged { change, .. } = TestApp::next_event(&mut events).await else {
        panic!("expected a sampling event");
    };
    assert_eq!(change, "settings");
}

#[tokio::test]
async fn a_bad_replacement_names_its_field_and_changes_nothing() {
    let app = TestApp::new().await;
    let before = app.state.config.snapshot_holder().load().version;
    for (changes, field) in [
        (json!({ "baselinePerMille": 5000 }), "baselinePerMille"),
        (json!({ "retentionDays": 0 }), "retentionDays"),
        (json!({ "maxBytes": 10 }), "maxBytes"),
        (json!({ "bodyRequestCap": 70_000 }), "bodyRequestCap"),
        (json!({ "stormBurst": 0 }), "stormBurst"),
        (json!({ "redactHeaders": ["bad name"] }), "redactHeaders"),
        (json!({ "triggers": ["slow", "server_error"] }), "triggers"),
    ] {
        let (status, body) = app.put("/api/sampling", settings(changes.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{changes}: {body}");
        let fields = body["error"]["fields"].as_array().unwrap();
        assert!(
            fields.iter().any(|f| f["field"] == field),
            "{changes}: {body}"
        );
    }
    assert_eq!(app.state.config.snapshot_holder().load().version, before);
    assert!(audit_entries(&app, "sampling.update").await.is_empty());
}

#[tokio::test]
async fn a_replacement_must_give_every_setting_and_no_unknown_one() {
    let app = TestApp::new().await;
    let mut missing = settings(json!({}));
    missing.as_object_mut().unwrap().remove("redactHeaders");
    let (status, _) = app.put("/api/sampling", missing).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a redaction list is never reset by omission"
    );
    let (status, _) = app
        .put("/api/sampling", settings(json!({ "bufferSamples": 10 })))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn overrides_can_be_set_for_a_subsystem_and_a_route_and_are_live() {
    let app = TestApp::new().await;
    let (subsystem, route) = app.site("shop", "shop.example.com").await;
    let mut events = app.state.events.subscribe();

    let (status, put) = app
        .put(
            "/api/sampling/overrides",
            json!({ "subsystemId": subsystem, "baselinePerMille": 10, "captureBodies": true }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(put["data"]["scope"], "subsystem");
    assert_eq!(put["data"]["subsystem"], "shop");
    assert_eq!(put["data"]["captureBodies"], true);
    assert_eq!(put["data"]["setBy"], "test-admin");

    let (status, put) = app
        .put(
            "/api/sampling/overrides",
            json!({ "routeId": route, "enabled": false }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{put}");
    assert_eq!(put["data"]["scope"], "route");
    assert_eq!(
        (
            put["data"]["subsystem"].as_str(),
            put["data"]["route"].as_str()
        ),
        (Some("shop"), Some("/"))
    );

    let (_, read) = app.get("/api/sampling").await;
    assert_eq!(read["data"]["overrides"].as_array().unwrap().len(), 2);
    let live = app.state.config.snapshot_holder().load();
    let policy = &live.subsystems[&fast_proxy_model::SubsystemId(subsystem)]
        .routes
        .find("/")
        .unwrap()
        .route
        .sampling
        .policy;
    assert!(!policy.enabled, "the route's own override wins");
    assert!(
        policy.capture_bodies,
        "and the rest comes from the subsystem's"
    );
    assert_eq!(policy.baseline_per_mille, 10);

    assert_eq!(audit_entries(&app, "sampling.override.set").await.len(), 2);
    let Event::SamplingChanged { change, .. } = TestApp::next_event(&mut events).await else {
        panic!("expected a sampling event");
    };
    assert_eq!(change, "override");
}

#[tokio::test]
async fn an_override_needs_exactly_one_target_that_exists_and_valid_values() {
    let app = TestApp::new().await;
    let (subsystem, route) = app.site("shop", "shop.example.com").await;
    let (status, _) = app
        .put("/api/sampling/overrides", json!({ "baselinePerMille": 1 }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "no target");
    let (status, _) = app
        .put(
            "/api/sampling/overrides",
            json!({ "subsystemId": subsystem, "routeId": route }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "two targets");
    let (status, _) = app
        .put("/api/sampling/overrides", json!({ "subsystemId": 9999 }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .put("/api/sampling/overrides", json!({ "routeId": 9999 }))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = app
        .put(
            "/api/sampling/overrides",
            json!({ "subsystemId": subsystem, "baselinePerMille": 5000 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]["fields"][0]["field"] == "baselinePerMille",
        "{body}"
    );
    let (status, _) = app
        .put(
            "/api/sampling/overrides",
            json!({ "subsystemId": subsystem, "redactHeaders": ["x"] }),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an override cannot touch redaction"
    );
    assert!(audit_entries(&app, "sampling.override.set")
        .await
        .is_empty());
}

#[tokio::test]
async fn an_override_is_cleared_by_its_target_and_clearing_nothing_is_harmless() {
    let app = TestApp::new().await;
    let (subsystem, route) = app.site("shop", "shop.example.com").await;
    app.put(
        "/api/sampling/overrides",
        json!({ "subsystemId": subsystem, "enabled": false }),
    )
    .await;
    app.put(
        "/api/sampling/overrides",
        json!({ "routeId": route, "enabled": true }),
    )
    .await;
    let mut events = app.state.events.subscribe();

    let (status, body) = app
        .delete(&format!("/api/sampling/overrides?subsystemId={subsystem}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, read) = app.get("/api/sampling").await;
    assert_eq!(read["data"]["overrides"].as_array().unwrap().len(), 1);
    assert_eq!(read["data"]["overrides"][0]["scope"], "route");
    assert_eq!(
        audit_entries(&app, "sampling.override.clear").await.len(),
        1
    );
    assert!(matches!(
        TestApp::next_event(&mut events).await,
        Event::SamplingChanged { .. }
    ));

    let version = app.state.config.snapshot_holder().load().version;
    let (status, body) = app
        .delete(&format!("/api/sampling/overrides?subsystemId={subsystem}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        app.state.config.snapshot_holder().load().version,
        version,
        "nothing was written"
    );
    assert_eq!(
        audit_entries(&app, "sampling.override.clear").await.len(),
        1
    );
    assert_eq!(
        app.delete("/api/sampling/overrides").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.delete("/api/sampling/overrides?routeId=x").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_viewer_reads_the_settings_and_status_but_changes_nothing() {
    let mut app = TestApp::new().await;
    let (subsystem, _) = app.site("shop", "shop.example.com").await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    assert_eq!(app.get("/api/sampling").await.0, StatusCode::OK);
    assert_eq!(
        app.put("/api/sampling", settings(json!({}))).await.0,
        StatusCode::FORBIDDEN
    );
    for (method, uri, body) in [
        (
            Method::PUT,
            "/api/sampling/overrides",
            Some(json!({ "subsystemId": subsystem })),
        ),
        (
            Method::DELETE,
            "/api/sampling/overrides?subsystemId=1",
            None,
        ),
    ] {
        assert_eq!(app.call(method, uri, body).await.0, StatusCode::FORBIDDEN);
    }
    assert!(audit_entries(&app, "sampling.update").await.is_empty());
}

#[tokio::test]
async fn a_rollback_leaves_the_sampling_settings_alone() {
    let app = TestApp::new().await;
    let (_, _) = app.site("shop", "shop.example.com").await;
    let first = app.state.config.snapshot_holder().load().version;
    app.put("/api/sampling", settings(json!({ "retentionDays": 9 })))
        .await;
    let (status, body) = app
        .call(
            Method::POST,
            &format!("/api/revisions/{first}/rollback"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, read) = app.get("/api/sampling").await;
    assert_eq!(read["data"]["settings"]["retentionDays"], 9);
}
