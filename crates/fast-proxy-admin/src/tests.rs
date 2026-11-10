//! Router-level tests of the system endpoints and fallbacks: build the real
//! `Router` used by `main.rs` (including the access-log middleware) and
//! drive it in-process.

use axum::http::StatusCode;

use crate::test_support::TestApp;

#[tokio::test]
async fn health_reports_ok() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["version"], "0.0.0-test");
}

#[tokio::test]
async fn config_exposes_ui_section_camel_cased() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ui"]["appName"], "Fast Proxy");
    assert_eq!(body["ui"]["defaultTheme"], "auto");
    assert_eq!(body["version"], "0.0.0-test");
}

#[tokio::test]
async fn unknown_api_route_returns_json_404_not_the_spa_shell() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/this-does-not-exist").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

/// Non-API routes fall back to the embedded SPA. Whether `ui/dist` has
/// been built varies by environment (a fresh clone hasn't run `npm run
/// build` yet), so this only pins down the two contractually valid
/// outcomes rather than depending on the frontend build being present.
#[tokio::test]
async fn unknown_non_api_route_serves_spa_shell_or_the_not_built_notice() {
    let app = TestApp::new().await;
    let (status, _) = app.get("/some/deep/link").await;
    assert!(
        status == StatusCode::OK || status == StatusCode::SERVICE_UNAVAILABLE,
        "unexpected status: {status}"
    );
}

// ---- metrics and traffic ------------------------------------------------

#[tokio::test]
async fn prometheus_metrics_are_served_as_text_and_track_config_changes() {
    let app = TestApp::new().await;
    let (status, _) = app
        .post("/api/subsystems", serde_json::json!({ "name": "shop" }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    app.post("/api/subsystems", serde_json::json!({ "name": "shop" }))
        .await;

    let (status, content_type, text) = app.get_text("/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(content_type.starts_with("text/plain"), "{content_type}");
    assert!(content_type.contains("version=0.0.4"), "{content_type}");
    // Labelled families only appear once they have a sample (no proxy traffic here).
    assert!(text.contains("# TYPE fastproxy_config_version gauge"));
    assert!(!text.contains("fastproxy_requests_total"));
    assert!(text.contains("fastproxy_config_version 1"));
    assert!(text.contains("fastproxy_config_reloads_total{outcome=\"applied\"} 1"));
    assert!(text.contains("fastproxy_config_reloads_total{outcome=\"rejected\"} 1"));
}

#[tokio::test]
async fn traffic_is_unavailable_until_the_first_sample_then_served_camel_cased() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/traffic").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error"]["code"], "INTERNAL");

    let totals = fast_proxy_telemetry::TrafficTotals::default();
    let snapshot =
        crate::traffic::compute_snapshot(&totals, &totals, std::time::Duration::from_secs(2), 99);
    *app.state.latest_traffic.write().await = Some(snapshot);
    let (status, body) = app.get("/api/traffic").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["timestampMs"], 99);
    assert_eq!(body["total"]["requestsPerSec"], 0.0);
}

#[tokio::test]
async fn applied_config_changes_reach_websocket_clients_as_events() {
    use crate::events::Event;

    let app = TestApp::new().await;
    crate::config_bridge::spawn_config_bridge(app.state.clone());
    let mut events = app.state.events.subscribe();

    app.post("/api/subsystems", serde_json::json!({ "name": "shop" }))
        .await;

    let wait = std::time::Duration::from_secs(2);
    let Ok(Ok(Event::ConfigChanged {
        version, summary, ..
    })) = tokio::time::timeout(wait, events.recv()).await
    else {
        panic!("expected a configChanged event first");
    };
    assert_eq!(version, 1);
    assert_eq!(summary, "subsystem created: shop");
    let Ok(Ok(Event::Activity { kind, message, .. })) =
        tokio::time::timeout(wait, events.recv()).await
    else {
        panic!("expected an activity entry second");
    };
    assert_eq!(kind, "config");
    assert_eq!(message, "subsystem created: shop");
}

#[tokio::test]
async fn rejected_changes_produce_no_events() {
    let app = TestApp::new().await;
    app.post("/api/subsystems", serde_json::json!({ "name": "shop" }))
        .await;
    crate::config_bridge::spawn_config_bridge(app.state.clone());
    let mut events = app.state.events.subscribe();
    app.post("/api/subsystems", serde_json::json!({ "name": "shop" }))
        .await;
    let waited = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv()).await;
    assert!(
        waited.is_err(),
        "no event should be sent for a rejected change"
    );
}

#[tokio::test]
async fn endpoint_state_changes_reach_websocket_clients_as_events_and_activity() {
    use crate::events::Event;
    use fast_proxy_telemetry::{EndpointChange, EndpointEvent};

    let app = TestApp::new().await;
    crate::endpoint_bridge::spawn_endpoint_bridge(app.state.clone());
    let mut events = app.state.events.subscribe();

    app.state.endpoint_events.publish(EndpointEvent {
        subsystem: "shop".into(),
        upstream: "backend".into(),
        endpoint: "10.0.0.1:80".into(),
        change: EndpointChange::Unhealthy {
            reason: "status 500 outside 200-399".into(),
        },
    });

    let wait = std::time::Duration::from_secs(2);
    let Ok(Ok(Event::EndpointState {
        subsystem,
        upstream,
        endpoint,
        change,
        detail,
        ..
    })) = tokio::time::timeout(wait, events.recv()).await
    else {
        panic!("expected an endpointState event first");
    };
    assert_eq!(
        (
            subsystem.as_str(),
            upstream.as_str(),
            endpoint.as_str(),
            change
        ),
        ("shop", "backend", "10.0.0.1:80", "unhealthy")
    );
    assert_eq!(detail.as_deref(), Some("status 500 outside 200-399"));
    let Ok(Ok(Event::Activity { kind, message, .. })) =
        tokio::time::timeout(wait, events.recv()).await
    else {
        panic!("expected an activity entry second");
    };
    assert_eq!(kind, "health");
    assert!(
        message.contains("10.0.0.1:80") && message.contains("is unhealthy"),
        "{message}"
    );
}
