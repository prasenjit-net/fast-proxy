//! Router-level integration tests: build the real `Router` used by
//! `main.rs` (including the access-log middleware) and drive it with
//! `tower::ServiceExt::oneshot`, the same way `axum::serve` would.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::{AdminState, UiConfig};
use fast_proxy_telemetry::AccessLog;

async fn test_app() -> Router {
    // No access-log path, so tests never touch disk.
    let access_log = AccessLog::open(None).await;
    let state = Arc::new(AdminState::new(
        UiConfig::default(),
        "0.0.0-test",
        access_log,
    ));
    crate::router(state)
}

/// The access-log middleware extracts `ConnectInfo<SocketAddr>`, which
/// `axum::serve(...).into_make_service_with_connect_info()` normally
/// inserts per-connection. Driving the router directly with `oneshot`
/// bypasses that, so tests insert it manually — otherwise every request
/// would fail extraction before reaching a handler.
fn request(method: Method, uri: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
    req
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_reports_ok() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::GET, "/api/health"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn config_exposes_ui_section_camel_cased() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::GET, "/api/config"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ui"]["appName"], "Fast Proxy");
    assert_eq!(body["ui"]["defaultTheme"], "auto");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn unknown_api_route_returns_json_404_not_the_spa_shell() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::GET, "/api/this-does-not-exist"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

/// Non-API routes fall back to the embedded SPA. Whether `ui/dist` has
/// been built varies by environment (a fresh clone hasn't run `npm run
/// build` yet), so this only pins down the two contractually valid
/// outcomes rather than depending on the frontend build being present.
#[tokio::test]
async fn unknown_non_api_route_serves_spa_shell_or_the_not_built_notice() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::GET, "/some/deep/link"))
        .await
        .unwrap();
    assert!(
        res.status() == StatusCode::OK || res.status() == StatusCode::SERVICE_UNAVAILABLE,
        "unexpected status: {}",
        res.status()
    );
}
