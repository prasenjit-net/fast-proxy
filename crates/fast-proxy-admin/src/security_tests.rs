//! The admin plane's protection, tested over HTTP: authentication, roles on
//! every registered route, the `Host` allow-list, the failure throttle, the
//! security headers, tokens and WebSocket tickets.

use std::net::SocketAddr;

use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use fast_proxy_auth::Principal;
use fast_proxy_model::{Action, Role, TokenId};
use http_body_util::BodyExt;
use serde_json::{json, Value};

use crate::guard::Access;
use crate::test_support::TestApp;

/// A concrete path for a route template: every `{param}` becomes `1`.
fn concrete(path: &str) -> String {
    let mut out = String::new();
    let mut in_param = false;
    for c in path.chars() {
        match c {
            '{' => {
                in_param = true;
                out.push('1');
            }
            '}' => in_param = false,
            c if !in_param => out.push(c),
            _ => {}
        }
    }
    out
}

/// The status of `method path`, sent from a different address each time so a
/// long walk over many routes never trips the failure throttle.
async fn status_of(app: &TestApp, method: &Method, path: &str, n: usize) -> StatusCode {
    let mut request = app
        .request_builder(method.clone(), &concrete(path))
        .body(Body::empty())
        .unwrap();
    let peer = SocketAddr::from(([10, 1, u8::try_from(n / 250).unwrap(), (n % 250) as u8], 1));
    request.extensions_mut().insert(ConnectInfo(peer));
    app.send(request).await.status()
}

async fn body_of(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

fn from_peer(app: &TestApp, peer: &str, token: Option<&str>) -> Request<Body> {
    let mut request = Request::builder()
        .uri("/api/status")
        .header("host", "localhost:9000");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
    let _ = app;
    request
}

// ---- A1: authentication and authorization ----

#[tokio::test]
async fn exactly_the_health_check_is_open_and_every_other_route_declares_an_action() {
    let app = TestApp::new().await;
    let open: Vec<&str> = app
        .specs
        .iter()
        .filter(|spec| spec.access == Access::Open)
        .map(|spec| spec.path)
        .collect();
    assert_eq!(open, ["/api/health"]);
    assert!(app.specs.len() > 45, "the table lists the real routes");
}

#[tokio::test]
async fn every_protected_route_refuses_a_missing_token_with_401() {
    let mut app = TestApp::new().await;
    app.sign_out();
    for (n, spec) in app.specs.clone().into_iter().enumerate() {
        if spec.access == Access::Open {
            continue;
        }
        let status = status_of(&app, &spec.method, spec.path, n).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{} {}",
            spec.method,
            spec.path
        );
    }
}

#[tokio::test]
async fn a_wrong_token_is_a_401_too_and_names_the_scheme() {
    let mut app = TestApp::new().await;
    app.sign_in_with("fp_1.not-the-secret".into());
    let request = app
        .request_builder(Method::GET, "/api/status")
        .body(Body::empty())
        .unwrap();
    let response = app.send(request).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
    let body = body_of(response).await;
    assert_eq!(body["error"]["code"], "UNAUTHORIZED");
}

#[tokio::test]
async fn a_viewer_is_refused_every_route_that_is_not_a_read() {
    let mut app = TestApp::new().await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    let mut writes_checked = 0;
    for (n, spec) in app.specs.clone().into_iter().enumerate() {
        let status = status_of(&app, &spec.method, spec.path, n).await;
        match spec.access {
            Access::Open => {}
            Access::Needs(Action::Read) => assert!(
                status != StatusCode::UNAUTHORIZED && status != StatusCode::FORBIDDEN,
                "a viewer may read: {} {} gave {status}",
                spec.method,
                spec.path
            ),
            Access::Needs(_) => {
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "{} {}",
                    spec.method,
                    spec.path
                );
                writes_checked += 1;
            }
            // A ticket route does not take a bearer token at all.
            Access::Ticket(_) => assert_eq!(status, StatusCode::UNAUTHORIZED),
        }
    }
    assert!(writes_checked > 25, "walked {writes_checked} write routes");
}

#[tokio::test]
async fn a_forbidden_write_changes_nothing() {
    let mut app = TestApp::new().await;
    let viewer = app.token("eyes", Role::Viewer).await;
    let before = app.state.config.snapshot_holder().load().version;
    app.sign_in_with(viewer);
    let (status, _) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(app.state.config.snapshot_holder().load().version, before);
}

#[tokio::test]
async fn an_expired_token_is_refused_without_any_write() {
    let mut app = TestApp::new().await;
    let created = app
        .config
        .create_token(
            &fast_proxy_config_service::ChangeContext::system(),
            "short",
            Role::Viewer,
            Some(crate::clock::now_ms() + 150),
        )
        .await
        .unwrap()
        .value;
    app.sign_in_with(created.token.reveal().to_string());
    assert_eq!(app.get("/api/status").await.0, StatusCode::OK);
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(app.get("/api/status").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_is_open_without_a_token() {
    let mut app = TestApp::new().await;
    app.sign_out();
    assert_eq!(app.get("/api/health").await.0, StatusCode::OK);
}

#[tokio::test]
async fn with_authentication_off_no_token_is_needed_and_the_caller_is_anonymous() {
    let mut app = TestApp::with_auth_off().await;
    app.sign_out();
    let (status, body) = app.get("/api/whoami").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["name"], "anonymous");
    assert_eq!(body["data"]["auth"], "off");
}

// ---- the Host allow-list ----

#[tokio::test]
async fn a_foreign_or_missing_host_is_refused_with_421_before_anything_else() {
    let app = TestApp::new().await;
    for host in [Some("evil.example"), Some("evil.example:9000"), None] {
        let mut builder = Request::builder().uri("/api/health");
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        let response = app.send(builder.body(Body::empty()).unwrap()).await;
        assert_eq!(
            response.status(),
            StatusCode::MISDIRECTED_REQUEST,
            "{host:?}"
        );
        assert!(response.headers().contains_key("x-content-type-options"));
    }
}

#[tokio::test]
async fn loopback_host_names_are_accepted_with_any_port() {
    let app = TestApp::new().await;
    for host in [
        "localhost",
        "localhost:1234",
        "127.0.0.1:9000",
        "[::1]:9000",
    ] {
        let request = Request::builder()
            .uri("/api/health")
            .header("host", host)
            .body(Body::empty())
            .unwrap();
        assert_eq!(app.send(request).await.status(), StatusCode::OK, "{host}");
    }
}

// ---- the failure throttle ----

#[tokio::test]
async fn ten_failures_from_one_address_throttle_it_even_for_a_valid_token() {
    let app = TestApp::new().await;
    let valid = app.token.clone().unwrap();
    for attempt in 0..10 {
        let response = app
            .send(from_peer(&app, "198.51.100.7:1", Some("fp_1.wrong")))
            .await;
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "attempt {attempt}"
        );
    }
    let response = app
        .send(from_peer(&app, "198.51.100.7:1", Some("fp_1.wrong")))
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry: u64 = response.headers()["retry-after"]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry));

    let response = app
        .send(from_peer(&app, "198.51.100.7:1", Some(&valid)))
        .await;
    assert_eq!(
        response.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "a flood cannot be used to guess faster"
    );
    let response = app
        .send(from_peer(&app, "198.51.100.8:1", Some(&valid)))
        .await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "another address is unaffected"
    );
}

#[tokio::test]
async fn successful_requests_never_count_as_failures() {
    let app = TestApp::new().await;
    let valid = app.token.clone().unwrap();
    for _ in 0..30 {
        let response = app
            .send(from_peer(&app, "198.51.100.9:1", Some(&valid)))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
}

// ---- security headers and request ids ----

#[tokio::test]
async fn api_ui_and_error_responses_carry_the_security_headers() {
    let app = TestApp::new().await;
    let mut checked = 0;
    for (uri, token) in [
        ("/api/status", true),
        ("/api/status", false),
        ("/", false),
        ("/api/health", false),
    ] {
        let mut builder = Request::builder().uri(uri).header("host", "localhost");
        if token {
            builder = builder.header(
                "authorization",
                format!("Bearer {}", app.token.clone().unwrap()),
            );
        }
        let response = app.send(builder.body(Body::empty()).unwrap()).await;
        let headers = response.headers();
        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("script-src 'self';"), "{uri}");
        assert_eq!(headers["x-content-type-options"], "nosniff", "{uri}");
        assert_eq!(headers["x-frame-options"], "DENY", "{uri}");
        assert_eq!(headers["referrer-policy"], "no-referrer", "{uri}");
        checked += 1;
    }
    assert_eq!(checked, 4);
}

#[tokio::test]
async fn the_api_is_never_cached_but_the_ui_may_be() {
    let app = TestApp::new().await;
    let token = app.token.clone().unwrap();
    let api = app
        .send(
            Request::builder()
                .uri("/api/status")
                .header("host", "localhost")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(api.headers()["cache-control"], "no-store");
    let denied = app
        .send(
            Request::builder()
                .uri("/api/status")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(denied.headers()["cache-control"], "no-store");
    let page = app
        .send(
            Request::builder()
                .uri("/")
                .header("host", "localhost")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert!(
        !page.headers().contains_key("cache-control")
            || page.headers()["cache-control"] != "no-store"
    );
}

#[tokio::test]
async fn every_response_has_a_distinct_request_id() {
    let app = TestApp::new().await;
    let mut ids = std::collections::HashSet::new();
    for _ in 0..5 {
        let request = app
            .request_builder(Method::GET, "/api/health")
            .body(Body::empty())
            .unwrap();
        let response = app.send(request).await;
        ids.insert(
            response.headers()["x-request-id"]
                .to_str()
                .unwrap()
                .to_string(),
        );
    }
    assert_eq!(ids.len(), 5);
}

// ---- A2: tokens ----

#[tokio::test]
async fn a_created_token_works_and_its_secret_never_appears_again() {
    let mut app = TestApp::new().await;
    let (status, created) = app
        .post(
            "/api/tokens",
            json!({ "name": "ci", "role": "viewer", "expiresInDays": 30 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let secret = created["data"]["secret"].as_str().unwrap().to_string();
    assert!(secret.starts_with("fp_"));
    assert_eq!(created["data"]["role"], "viewer");
    assert_eq!(created["data"]["name"], "ci");
    assert!(created["data"]["expiresAtMs"].as_i64().unwrap() > crate::clock::now_ms());

    let bare_secret = secret.rsplit('.').next().unwrap().to_string();
    let mut texts = vec![
        app.get("/api/tokens").await.1.to_string(),
        app.get("/api/status").await.1.to_string(),
        app.get("/api/revisions").await.1.to_string(),
        app.get("/api/whoami").await.1.to_string(),
        app.get_text("/metrics").await.2,
    ];
    let audit = app
        .config
        .read(|conn| {
            fast_proxy_store::repository::audit::list(
                conn,
                &fast_proxy_store::repository::audit::AuditQuery::default(),
                None,
                100,
            )
        })
        .await
        .unwrap()
        .value;
    texts.push(format!("{audit:?}"));
    for text in texts {
        assert!(!text.contains(&bare_secret), "the secret leaked");
    }

    app.sign_in_with(secret);
    let (status, who) = app.get("/api/whoami").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(who["data"]["name"], "ci");
    assert_eq!(who["data"]["role"], "viewer");
    assert!(who["data"]["tokenExpiresAtMs"].is_i64());
}

#[tokio::test]
async fn revoking_a_token_takes_effect_on_the_next_request() {
    let mut app = TestApp::new().await;
    let (_, created) = app
        .post("/api/tokens", json!({ "name": "temp", "role": "admin" }))
        .await;
    let id = created["data"]["id"].as_i64().unwrap();
    let secret = created["data"]["secret"].as_str().unwrap().to_string();
    let (status, _) = app
        .call(Method::DELETE, &format!("/api/tokens/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    app.sign_in_with(secret);
    assert_eq!(app.get("/api/status").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_last_admin_token_cannot_be_revoked_over_the_api() {
    let app = TestApp::new().await;
    let (_, list) = app.get("/api/tokens").await;
    let id = list["data"][0]["id"].as_i64().unwrap();
    let (status, body) = app
        .call(Method::DELETE, &format!("/api/tokens/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(app.get("/api/status").await.0, StatusCode::OK);
}

#[tokio::test]
async fn token_requests_are_validated_field_by_field() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/api/tokens",
            json!({ "name": "", "role": "root", "expiresInDays": 0 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let fields: Vec<&str> = body["error"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["name", "role", "expiresInDays"]);
    let (status, _) = app
        .post(
            "/api/tokens",
            json!({ "name": "x", "role": "admin", "expiresInDays": 99999 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_duplicate_token_name_is_a_conflict_and_an_unknown_id_a_404() {
    let app = TestApp::new().await;
    let make = || app.post("/api/tokens", json!({ "name": "dup", "role": "viewer" }));
    assert_eq!(make().await.0, StatusCode::CREATED);
    assert_eq!(make().await.0, StatusCode::CONFLICT);
    let (status, _) = app.call(Method::DELETE, "/api/tokens/999", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_token_list_shows_last_use_and_never_a_hash() {
    let app = TestApp::new().await;
    let (_, list) = app.get("/api/tokens").await;
    let first = &list["data"][0];
    for key in [
        "id",
        "name",
        "role",
        "createdAtMs",
        "createdBy",
        "revokedAtMs",
        "lastUsedAtMs",
    ] {
        assert!(first.get(key).is_some(), "{key}");
    }
    let text = list.to_string().to_lowercase();
    assert!(!text.contains("hash") && !text.contains("secret"));
}

// ---- A3: WebSocket tickets ----

fn ws_request(app: &TestApp, query: &str) -> Request<Body> {
    app.request_builder(Method::GET, &format!("/ws{query}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn a_ticket_is_single_use_and_a_missing_one_is_refused() {
    let mut app = TestApp::new().await;
    let (status, issued) = app.call(Method::POST, "/api/ws-ticket", None).await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    assert_eq!(issued["expiresInSecs"], 30);
    let ticket = issued["ticket"].as_str().unwrap().to_string();

    // The bearer header alone does not open the socket.
    assert_eq!(
        app.send(ws_request(&app, "")).await.status(),
        StatusCode::UNAUTHORIZED
    );
    app.sign_out();
    let first = app
        .send(ws_request(&app, &format!("?ticket={ticket}")))
        .await;
    assert_ne!(
        first.status(),
        StatusCode::UNAUTHORIZED,
        "the ticket got past the guard"
    );
    assert_ne!(first.status(), StatusCode::FORBIDDEN);
    let second = app
        .send(ws_request(&app, &format!("?ticket={ticket}")))
        .await;
    assert_eq!(second.status(), StatusCode::UNAUTHORIZED, "used twice");
}

#[tokio::test]
async fn an_expired_or_unknown_ticket_is_refused() {
    let app = TestApp::new().await;
    let principal = Principal {
        token_id: TokenId(1),
        name: "test-admin".into(),
        role: Role::Admin,
        expires_at_ms: None,
    };
    let stale = app
        .state
        .security
        .tickets
        .issue(principal, crate::clock::now_ms() - 60_000)
        .unwrap();
    assert_eq!(
        app.send(ws_request(&app, &format!("?ticket={stale}")))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.send(ws_request(&app, "?ticket=made-up")).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_viewer_can_get_a_ticket_and_open_the_socket() {
    let mut app = TestApp::new().await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    let (status, issued) = app.call(Method::POST, "/api/ws-ticket", None).await;
    assert_eq!(status, StatusCode::OK);
    let ticket = issued["ticket"].as_str().unwrap();
    let response = app
        .send(ws_request(&app, &format!("?ticket={ticket}")))
        .await;
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    assert_ne!(response.status(), StatusCode::FORBIDDEN);
}

// ---- A7: reload and status ----

#[tokio::test]
async fn reload_answers_with_the_live_version() {
    let app = TestApp::new().await;
    app.post("/api/subsystems", json!({ "name": "shop" })).await;
    let (status, body) = app.call(Method::POST, "/api/reload", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["version"],
        app.state.config.snapshot_holder().load().version
    );
}

#[tokio::test]
async fn status_reports_the_protection_tokens_and_maintenance() {
    let app = TestApp::new().await;
    let (_, body) = app.get("/api/status").await;
    let admin = &body["data"]["admin"];
    assert_eq!(admin["auth"], "required");
    assert_eq!(admin["protectedChannel"], true);
    assert_eq!(admin["activeTokens"], 1);
    assert_eq!(admin["activeMaintenance"], 0);

    app.config
        .set_maintenance(
            &fast_proxy_config_service::ChangeContext::system(),
            fast_proxy_model::MaintenanceTarget::Global,
            fast_proxy_model::MaintenanceSettings {
                message: "x".into(),
                body: None,
                retry_after_secs: None,
                bypass: fast_proxy_model::BypassRules::default(),
                expires_at_ms: None,
                set_by: String::new(),
                set_at_ms: 0,
            },
        )
        .await
        .unwrap();
    app.post("/api/tokens", json!({ "name": "more", "role": "viewer" }))
        .await;
    let (_, body) = app.get("/api/status").await;
    assert_eq!(body["data"]["admin"]["activeMaintenance"], 1);
    assert_eq!(body["data"]["admin"]["activeTokens"], 2);
}

#[tokio::test]
async fn whoami_reports_a_channel_that_is_not_protected() {
    let app = TestApp::with_unprotected_channel().await;
    let (_, body) = app.get("/api/whoami").await;
    assert_eq!(body["data"]["protectedChannel"], false);
}

#[tokio::test]
async fn a_reload_of_an_unloadable_database_is_a_409_and_the_old_configuration_keeps_serving() {
    let app = TestApp::new().await;
    app.post("/api/subsystems", json!({ "name": "shop" })).await;
    app.config
        .read(|conn| {
            conn.execute(
                "INSERT INTO subsystem_host (subsystem_id, pattern) VALUES (1, 'not a host!')",
                [],
            )
            .map_err(fast_proxy_store::StoreError::from)
        })
        .await
        .unwrap();
    let (status, body) = app.call(Method::POST, "/api/reload", None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        !message.contains("not a host"),
        "storage detail stays in the log"
    );
    assert_eq!(
        app.state.config.snapshot_holder().load().subsystems.len(),
        1,
        "the live snapshot is untouched"
    );
}

#[tokio::test]
async fn too_many_outstanding_tickets_answer_429() {
    let app = TestApp::new().await;
    let mut last = StatusCode::OK;
    for _ in 0..1_100 {
        last = app.call(Method::POST, "/api/ws-ticket", None).await.0;
        if last == StatusCode::TOO_MANY_REQUESTS {
            break;
        }
    }
    assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
}
