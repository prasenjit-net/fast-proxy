//! A running admin router over an in-memory store, driven in-process.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use fast_proxy_config_service::ConfigService;
use fast_proxy_store::Store;
use fast_proxy_telemetry::AccessLog;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::{AdminState, UiConfig};

pub struct TestApp {
    router: Router,
    pub config: Arc<ConfigService>,
}

impl TestApp {
    pub async fn new() -> Self {
        let config = Arc::new(
            ConfigService::start(Store::open_in_memory().unwrap())
                .await
                .unwrap(),
        );
        // No access-log path, so tests never touch disk.
        let access_log = AccessLog::open(None).await;
        let state = Arc::new(AdminState::new(
            UiConfig::default(),
            "0.0.0-test",
            access_log,
            config.clone(),
        ));
        Self {
            router: crate::router(state),
            config,
        }
    }

    /// Sends a request with a raw body and returns the status and the JSON
    /// body (`Null` when the body is empty or not JSON).
    ///
    /// The access-log middleware extracts `ConnectInfo<SocketAddr>`, which
    /// `axum::serve(...).into_make_service_with_connect_info()` normally
    /// inserts per connection; driving the router with `oneshot` bypasses
    /// that, so it is inserted here.
    pub async fn call_raw(
        &self,
        method: Method,
        uri: &str,
        body: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let mut request = request
            .body(body.map_or_else(Body::empty, |text| Body::from(text.to_string())))
            .unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    pub async fn call(
        &self,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.call_raw(method, uri, body.map(|v| v.to_string()).as_deref())
            .await
    }

    pub async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.call(Method::GET, uri, None).await
    }

    pub async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::POST, uri, Some(body)).await
    }

    pub async fn put(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.call(Method::PUT, uri, Some(body)).await
    }

    pub async fn delete(&self, uri: &str) -> (StatusCode, Value) {
        self.call(Method::DELETE, uri, None).await
    }
}
