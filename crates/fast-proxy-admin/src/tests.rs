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
