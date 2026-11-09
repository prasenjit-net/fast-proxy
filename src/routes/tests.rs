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

use crate::config::AppConfig;
use crate::state::AppState;

async fn test_app() -> Router {
    // AppConfig::default() has no access_log path, so tests never touch disk.
    let state = Arc::new(AppState::new(AppConfig::default()).await);
    crate::routes::router(state)
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

fn json_request(method: Method, uri: &str, body: Value) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
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
    assert_eq!(body["ui"]["appName"], "Rusty Template");
    assert_eq!(body["ui"]["defaultTheme"], "auto");
    assert!(body["version"].is_string());
}

#[tokio::test]
async fn tasks_seed_list_has_three_items() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::GET, "/api/tasks"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn create_task_then_list_reflects_it() {
    let app = test_app().await;

    let create = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/tasks",
            serde_json::json!({ "title": "Write tests" }),
        ))
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::CREATED);
    let created = body_json(create).await;
    assert_eq!(created["title"], "Write tests");
    assert_eq!(created["done"], false);

    let list = app
        .oneshot(request(Method::GET, "/api/tasks"))
        .await
        .unwrap();
    let body = body_json(list).await;
    assert_eq!(body.as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn create_task_with_empty_title_is_rejected() {
    let app = test_app().await;
    let res = app
        .oneshot(json_request(
            Method::POST,
            "/api/tasks",
            serde_json::json!({ "title": "   " }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "BAD_REQUEST");
}

#[tokio::test]
async fn toggle_unknown_task_is_not_found() {
    let app = test_app().await;
    let res = app
        .oneshot(request(Method::POST, "/api/tasks/999/toggle"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = body_json(res).await;
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn toggle_then_delete_existing_task() {
    let app = test_app().await;

    let toggled = app
        .clone()
        .oneshot(request(Method::POST, "/api/tasks/1/toggle"))
        .await
        .unwrap();
    assert_eq!(toggled.status(), StatusCode::OK);
    let toggled_body = body_json(toggled).await;
    // Seed task 1 starts done=true (see TaskStore::with_examples).
    assert_eq!(toggled_body["done"], false);

    let deleted = app
        .clone()
        .oneshot(request(Method::DELETE, "/api/tasks/1"))
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);

    let list = app
        .oneshot(request(Method::GET, "/api/tasks"))
        .await
        .unwrap();
    let body = body_json(list).await;
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn error_demo_kinds_map_to_the_right_status_and_code() {
    let app = test_app().await;

    let bad = app
        .clone()
        .oneshot(request(Method::GET, "/api/error-demo?kind=bad-request"))
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    assert_eq!(body_json(bad).await["error"]["code"], "BAD_REQUEST");

    let missing = app
        .clone()
        .oneshot(request(Method::GET, "/api/error-demo?kind=not-found"))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(missing).await["error"]["code"], "NOT_FOUND");

    let internal = app
        .oneshot(request(Method::GET, "/api/error-demo?kind=internal"))
        .await
        .unwrap();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body_json(internal).await["error"]["code"], "INTERNAL");
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
