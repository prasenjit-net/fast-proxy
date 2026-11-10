//! A running admin router over an in-memory store, driven in-process.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use axum::Router;
use fast_proxy_auth::{GeneratedSecret, PlaintextToken};
use fast_proxy_config_service::{ChangeContext, ConfigService, TlsSettings};
use fast_proxy_model::{Role, TokenName};
use fast_proxy_store::repository::tokens::{self, NewToken};
use fast_proxy_store::Store;
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::{
    AdminSecurity, AdminState, ListenerInfo, RuntimeInfo, SecuritySettings, SharedState, TlsInfo,
    UiConfig,
};

/// How the admin plane under test is protected.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Protection {
    Normal,
    UnprotectedChannel,
    AuthOff,
    /// Serving TLS with the stored certificate `admin`.
    AdminTls,
}

pub struct TestApp {
    router: Router,
    pub config: Arc<ConfigService>,
    pub metrics: Arc<Metrics>,
    pub state: SharedState,
    /// The bearer token sent with every request; `None` sends none.
    pub token: Option<String>,
    /// Every route the router registered, with the access it needs.
    pub specs: Vec<crate::route_table::RouteSpec>,
}

/// A master key for tests that store private keys (no key file is created).
const TEST_MASTER_KEY: &str = "BQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQU=";

impl TestApp {
    /// TLS off, and no master key, so no private key can be stored.
    pub async fn new() -> Self {
        Self::start(TlsSettings::default(), Protection::Normal).await
    }

    /// The same, with the admin plane's channel not safe for private keys (as
    /// when it listens beyond loopback without TLS).
    pub async fn with_unprotected_channel() -> Self {
        Self::start(
            TlsSettings {
                enabled: true,
                master_key_file: None,
                env_master_key: Some(TEST_MASTER_KEY.into()),
            },
            Protection::UnprotectedChannel,
        )
        .await
    }

    /// The same, with the admin plane serving the stored certificate `admin`
    /// over TLS (the server itself is not started; only the rules apply).
    pub async fn with_admin_tls() -> Self {
        Self::start(
            TlsSettings {
                enabled: false,
                master_key_file: None,
                env_master_key: Some(TEST_MASTER_KEY.into()),
            },
            Protection::AdminTls,
        )
        .await
    }

    /// The same, with authentication switched off (`auth = "off"`).
    pub async fn with_auth_off() -> Self {
        Self::start(TlsSettings::default(), Protection::AuthOff).await
    }

    /// Creates another token and returns its text, for tests of roles.
    pub async fn token(&self, name: &str, role: Role) -> String {
        self.config
            .create_token(&ChangeContext::system(), name, role, None)
            .await
            .unwrap()
            .value
            .token
            .reveal()
            .to_string()
    }

    /// Sends no token from now on.
    pub fn sign_out(&mut self) {
        self.token = None;
    }

    /// Sends `token` from now on.
    pub fn sign_in_with(&mut self, token: String) {
        self.token = Some(token);
    }

    /// A master key from the environment form, with TLS on or off.
    pub async fn with_tls(enabled: bool) -> Self {
        Self::start(
            TlsSettings {
                enabled,
                master_key_file: None,
                env_master_key: Some(TEST_MASTER_KEY.into()),
            },
            Protection::Normal,
        )
        .await
    }

    async fn start(settings: TlsSettings, protection: Protection) -> Self {
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
        // The admin token is stored before the service starts (which loads
        // tokens), so it leaves the configuration version at 0.
        let store = Store::open_in_memory().unwrap();
        let secret = GeneratedSecret::generate().unwrap();
        let new_token = NewToken {
            id: None,
            name: TokenName::parse("test-admin").unwrap(),
            role: Role::Admin,
            secret_hash: secret.hash().clone(),
            created_by: "test".into(),
            expires_at_ms: None,
        };
        let id = store
            .run(move |db| db.read(|conn| tokens::create(conn, &new_token, 0)))
            .await
            .unwrap();
        let admin_token = PlaintextToken::new(id, &secret).reveal().to_string();
        let config = Arc::new(
            ConfigService::start_with_tls(store, metrics.clone(), settings)
                .await
                .unwrap(),
        );
        let security = AdminSecurity::new(
            &SecuritySettings {
                auth_required: protection != Protection::AuthOff,
                channel_protected: protection != Protection::UnprotectedChannel,
                listen: SocketAddr::from(([127, 0, 0, 1], 9000)),
                allowed_hosts: vec![],
                admin_certificate: (protection == Protection::AdminTls).then(|| "admin".into()),
            },
            config.token_cache(),
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
            security,
        ));
        let (router, specs) = crate::router::build(state.clone());
        Self {
            router,
            specs,
            config,
            metrics,
            state,
            token: Some(admin_token),
        }
    }

    /// A request to `uri` as a browser on the admin origin would send it: the
    /// `Host` of the listen address and the current bearer token, if any.
    pub fn request_builder(&self, method: Method, uri: &str) -> axum::http::request::Builder {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost:9000");
        if let Some(token) = &self.token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        request
    }

    /// The router under test, to serve on a real listener.
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Sends a prepared request (from [`TestApp::request_builder`]) and returns
    /// the whole response, headers included. A peer address is added when the
    /// request has none.
    pub async fn send(&self, mut request: Request<Body>) -> axum::response::Response {
        if request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_none()
        {
            request
                .extensions_mut()
                .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        }
        self.router.clone().oneshot(request).await.unwrap()
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
        let mut request = self.request_builder(method, uri);
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
        let mut request = self
            .request_builder(Method::GET, uri)
            .body(Body::empty())
            .unwrap();
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
