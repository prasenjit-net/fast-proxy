//! A running admin router over an in-memory store, driven in-process.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use fast_proxy_config_service::{ConfigService, TlsSettings};
use fast_proxy_store::Store;
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::{AdminState, ListenerInfo, RuntimeInfo, SharedState, TlsInfo, UiConfig};

pub struct TestApp {
    router: Router,
    pub config: Arc<ConfigService>,
    pub metrics: Arc<Metrics>,
    pub state: SharedState,
}

/// A master key for tests that store private keys (no key file is created).
const TEST_MASTER_KEY: &str = "BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU=";

impl TestApp {
    /// TLS off, and no master key, so no private key can be stored.
    pub async fn new() -> Self {
        Self::start(TlsSettings::default()).await
    }

    /// A master key from the environment form, with TLS on or off.
    pub async fn with_tls(enabled: bool) -> Self {
        Self::start(TlsSettings {
            enabled,
            master_key_file: None,
            env_master_key: Some(TEST_MASTER_KEY.into()),
        })
        .await
    }

    async fn start(settings: TlsSettings) -> Self {
        let tls = TlsInfo {
            enabled: settings.enabled,
            listen: settings
                .enabled
                .then(|| SocketAddr::from(([127, 0, 0, 1], 8443))),
            http2: false,
            min_version: "1.2".into(),
            redirect_port: 8443,
        };
        let metrics = Arc::new(Metrics::new());
        let config = Arc::new(
            ConfigService::start_with_tls(
                Store::open_in_memory().unwrap(),
                metrics.clone(),
                settings,
            )
            .await
            .unwrap(),
        );
        // No access-log path, so tests never touch disk.
        let access_log = Arc::new(AccessLog::open(None).await);
        let state = Arc::new(AdminState::new(
            UiConfig::default(),
            "0.0.0-test",
            access_log,
            config.clone(),
            RuntimeInfo {
                listeners: vec![ListenerInfo {
                    name: "http".into(),
                    addr: SocketAddr::from(([127, 0, 0, 1], 8000)),
                }],
                database_path: "/tmp/test.db".into(),
                tls,
            },
            metrics.clone(),
            EndpointEvents::new(),
        ));
        Self {
            router: crate::router(state.clone()),
            config,
            metrics,
            state,
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
        self.call_raw_from(SocketAddr::from(([127, 0, 0, 1], 0)), method, uri, body)
            .await
    }

    /// Like [`TestApp::call_raw`], as a request coming from `peer`.
    pub async fn call_raw_from(
        &self,
        peer: SocketAddr,
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
        request.extensions_mut().insert(ConnectInfo(peer));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// A request with a JSON body from a chosen peer address.
    pub async fn call_from(
        &self,
        peer: &str,
        method: Method,
        uri: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let peer: SocketAddr = peer.parse().unwrap();
        self.call_raw_from(peer, method, uri, body.map(|v| v.to_string()).as_deref())
            .await
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

    /// A `GET` whose body is text, returning the status, content type, and body.
    pub async fn get_text(&self, uri: &str) -> (StatusCode, String, String) {
        let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            content_type,
            String::from_utf8(bytes.to_vec()).unwrap(),
        )
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
