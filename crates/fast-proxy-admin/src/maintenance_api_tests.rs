//! The maintenance API: each scope's set, update and clear, validation, the
//! bypass secret shown once, the events, the resolver, and that each write is
//! live in the data plane's snapshot without a restart.

use axum::http::{Method, StatusCode};
use fast_proxy_model::MaintenanceTarget;
use serde_json::{json, Value};

use crate::test_support::TestApp;

/// A subsystem with a host, an upstream and routes `/` and `/api`; returns the
/// subsystem id and the `/api` route id.
async fn site(app: &TestApp, name: &str, host: &str) -> (i64, i64) {
    let (status, body) = app.post("/api/subsystems", json!({ "name": name })).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = body["data"]["id"].as_i64().unwrap();
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/hosts"),
            json!({ "pattern": host }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({ "name": "backend", "endpoints": [{ "addr": "127.0.0.1:3000" }] }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let upstream = body["data"]["id"].as_i64().unwrap();
    for prefix in ["/", "/api"] {
        let (status, _) = app
            .post(
                &format!("/api/subsystems/{id}/routes"),
                json!({ "prefix": prefix, "upstreamGroupId": upstream }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let (_, routes) = app.get(&format!("/api/subsystems/{id}/routes")).await;
    let api = routes["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["prefix"] == "/api")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    (id, api)
}

fn live(app: &TestApp) -> std::sync::Arc<fast_proxy_routing::Snapshot> {
    app.state.config.snapshot_holder().load()
}

async fn put(app: &TestApp, uri: &str, body: Value) -> (StatusCode, Value) {
    app.call(Method::PUT, uri, Some(body)).await
}

async fn delete(app: &TestApp, uri: &str) -> (StatusCode, Value) {
    app.call(Method::DELETE, uri, None).await
}

fn fields(body: &Value) -> Vec<String> {
    body["error"]["fields"]
        .as_array()
        .map(|all| {
            all.iter()
                .map(|f| f["field"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---- each scope: set, update, clear, live ----

#[tokio::test]
async fn the_global_scope_is_set_updated_and_cleared_and_live_each_time() {
    let app = TestApp::new().await;
    let (status, body) = put(
        &app,
        "/api/maintenance/global",
        json!({ "message": "deploying" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["scope"], "global");
    assert_eq!(body["data"]["message"], "deploying");
    assert_eq!(body["data"]["setBy"], "test-admin");
    assert!(live(&app).maintenance.is_some(), "live without a restart");

    let (_, body) = put(
        &app,
        "/api/maintenance/global",
        json!({ "message": "still deploying" }),
    )
    .await;
    assert_eq!(body["data"]["message"], "still deploying");
    assert_eq!(
        app.get("/api/maintenance").await.1["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let (status, _) = delete(&app, "/api/maintenance/global").await;
    assert_eq!(status, StatusCode::OK);
    assert!(live(&app).maintenance.is_none());
    assert!(app.get("/api/maintenance").await.1["data"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_subsystem_scope_is_set_and_cleared() {
    let app = TestApp::new().await;
    let (shop, _) = site(&app, "shop", "shop.example.com").await;
    let uri = format!("/api/subsystems/{shop}/maintenance");
    let (status, body) = put(&app, &uri, json!({ "message": "x", "ttlSecs": 600 })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["scope"], "subsystem");
    assert_eq!(body["data"]["subsystem"], "shop");
    let left = body["data"]["secondsLeft"].as_i64().unwrap();
    assert!((595..=600).contains(&left), "{left}");
    assert!(live(&app)
        .subsystems
        .values()
        .any(|s| s.maintenance.is_some()));
    delete(&app, &uri).await;
    assert!(live(&app)
        .subsystems
        .values()
        .all(|s| s.maintenance.is_none()));
}

#[tokio::test]
async fn a_route_scope_names_its_route_and_is_set_and_cleared() {
    let app = TestApp::new().await;
    let (shop, api) = site(&app, "shop", "shop.example.com").await;
    let uri = format!("/api/subsystems/{shop}/routes/{api}/maintenance");
    let (status, body) = put(&app, &uri, json!({ "message": "api down" })).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["scope"], "route");
    assert_eq!(body["data"]["route"], "/api");
    assert_eq!(body["data"]["subsystem"], "shop");
    let (_, list) = app.get("/api/maintenance").await;
    assert_eq!(list["data"][0]["route"], "/api");
    delete(&app, &uri).await;
    assert!(app.get("/api/maintenance").await.1["data"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn unknown_targets_are_404_and_a_route_must_belong_to_its_subsystem() {
    let app = TestApp::new().await;
    let (shop, api) = site(&app, "shop", "shop.example.com").await;
    let (other, _) = site(&app, "blog", "blog.example.com").await;
    for (method, uri) in [
        (Method::PUT, "/api/subsystems/99/maintenance".to_string()),
        (Method::DELETE, "/api/subsystems/99/maintenance".to_string()),
        (
            Method::PUT,
            format!("/api/subsystems/{shop}/routes/9999/maintenance"),
        ),
        (
            Method::PUT,
            format!("/api/subsystems/{other}/routes/{api}/maintenance"),
        ),
        (
            Method::DELETE,
            format!("/api/subsystems/{other}/routes/{api}/maintenance"),
        ),
    ] {
        let body = (method == Method::PUT).then(|| json!({}));
        let (status, _) = app.call(method.clone(), &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

#[tokio::test]
async fn clearing_what_is_not_on_is_a_harmless_200() {
    let app = TestApp::new().await;
    let before = live(&app).version;
    let (status, body) = delete(&app, "/api/maintenance/global").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], before);
}

#[tokio::test]
async fn the_list_hides_a_scope_once_it_is_over() {
    let app = TestApp::new().await;
    put(&app, "/api/maintenance/global", json!({ "ttlSecs": 1 })).await;
    assert_eq!(
        app.get("/api/maintenance").await.1["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert!(app.get("/api/maintenance").await.1["data"]
        .as_array()
        .unwrap()
        .is_empty());
}

// ---- validation ----

#[tokio::test]
async fn invalid_settings_are_refused_field_by_field_and_change_nothing() {
    let app = TestApp::new().await;
    let (status, body) = put(
        &app,
        "/api/maintenance/global",
        json!({
            "retryAfterSecs": 0,
            "ttlSecs": 0,
            "bypass": { "cidrs": ["nope", "10.0.0.0/33"], "secret": "x" },
            "body": "<p>x</p>",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let names = fields(&body);
    for expected in ["bypass.cidrs", "ttlSecs", "bypass.secret", "contentType"] {
        assert!(
            names.contains(&expected.to_string()),
            "{expected} in {names:?}"
        );
    }
    assert!(live(&app).maintenance.is_none());
}

#[tokio::test]
async fn each_bound_is_checked() {
    let app = TestApp::new().await;
    let cases = [
        (json!({ "retryAfterSecs": 0 }), "retryAfterSecs"),
        (json!({ "retryAfterSecs": 86_401 }), "retryAfterSecs"),
        (json!({ "ttlSecs": 2_592_001 }), "ttlSecs"),
        (json!({ "contentType": "text/html" }), "body"),
        (
            json!({ "body": "x", "contentType": "image/png" }),
            "contentType",
        ),
        (
            json!({ "body": "a".repeat(16 * 1024 + 1), "contentType": "text/plain" }),
            "body",
        ),
        (json!({ "message": "m".repeat(1025) }), "message"),
        (
            json!({ "bypass": { "cidrs": vec!["10.0.0.0/8"; 33] } }),
            "bypass.cidrs",
        ),
        (
            json!({ "bypass": { "header": "bad header" } }),
            "bypass.header",
        ),
        (
            json!({ "bypass": { "secret": "0123456789abcdef0123" } }),
            "bypass.secret",
        ),
        (
            json!({ "bypass": { "header": "X-Pass", "secret": "short" } }),
            "bypass.secret",
        ),
    ];
    for (body, field) in cases {
        let (status, answer) = put(&app, "/api/maintenance/global", body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {answer}");
        assert!(
            fields(&answer).contains(&field.to_string()),
            "{body}: {answer}"
        );
    }
    assert!(live(&app).maintenance.is_none(), "nothing was stored");
}

#[tokio::test]
async fn unknown_keys_are_rejected() {
    let app = TestApp::new().await;
    let (status, _) = put(&app, "/api/maintenance/global", json!({ "mesage": "typo" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- the bypass secret ----

#[tokio::test]
async fn a_generated_secret_is_returned_once_and_matches_in_the_data_plane() {
    let app = TestApp::new().await;
    let (status, body) = put(
        &app,
        "/api/maintenance/global",
        json!({ "bypass": { "header": "X-Maintenance-Pass", "cidrs": ["10.0.0.0/8"] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let secret = body["bypassSecret"]
        .as_str()
        .expect("returned once")
        .to_string();
    assert_eq!(body["data"]["bypass"]["secretSet"], true);
    assert_eq!(body["data"]["bypass"]["header"], "X-Maintenance-Pass");
    assert_eq!(body["data"]["bypass"]["cidrs"], json!(["10.0.0.0/8"]));

    let snapshot = live(&app);
    let scope = snapshot.maintenance.as_ref().unwrap();
    let outsider = "203.0.113.9".parse().unwrap();
    assert_eq!(
        scope.check(0, outsider, Some(&secret)),
        fast_proxy_routing::MaintenanceCheck::Bypassed
    );
    assert_eq!(
        scope.check(0, outsider, Some("wrong")),
        fast_proxy_routing::MaintenanceCheck::Respond
    );

    let (_, list) = app.get("/api/maintenance").await;
    for text in [
        list.to_string(),
        app.get("/api/revisions").await.1.to_string(),
        app.get_text("/metrics").await.2,
    ] {
        assert!(!text.contains(&secret), "the secret leaked");
    }
    assert!(list["data"][0].get("bypassSecret").is_none());
}

#[tokio::test]
async fn a_secret_the_operator_chose_is_used_and_never_echoed() {
    let app = TestApp::new().await;
    let chosen = "my-own-bypass-secret-123";
    let (_, body) = put(
        &app,
        "/api/maintenance/global",
        json!({ "bypass": { "header": "X-Pass", "secret": chosen } }),
    )
    .await;
    assert!(body["bypassSecret"].is_null());
    assert!(!body.to_string().contains(chosen));
    let snapshot = live(&app);
    assert_eq!(
        snapshot.maintenance.as_ref().unwrap().check(
            0,
            "203.0.113.9".parse().unwrap(),
            Some(chosen)
        ),
        fast_proxy_routing::MaintenanceCheck::Bypassed
    );
}

// ---- events, audit ----

#[tokio::test]
async fn turning_maintenance_on_and_off_is_pushed_to_clients() {
    let app = TestApp::new().await;
    let (shop, _) = site(&app, "shop", "shop.example.com").await;
    let mut events = app.state.events.subscribe();
    let uri = format!("/api/subsystems/{shop}/maintenance");
    put(&app, &uri, json!({ "message": "x", "ttlSecs": 60 })).await;
    delete(&app, &uri).await;

    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        let json = serde_json::to_value(&event).unwrap();
        if json["type"] == "maintenanceChanged" {
            seen.push(json);
        }
    }
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0]["active"], true);
    assert_eq!(seen[0]["scope"], "subsystem");
    assert_eq!(seen[0]["subsystem"], "shop");
    assert!(seen[0]["expiresAtMs"].is_i64());
    assert_eq!(seen[1]["active"], false);
}

#[tokio::test]
async fn the_changes_are_audited_with_the_actor() {
    use fast_proxy_store::repository::audit::{self, AuditQuery};
    let app = TestApp::new().await;
    put(&app, "/api/maintenance/global", json!({})).await;
    delete(&app, "/api/maintenance/global").await;
    let entries = app
        .config
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 10))
        .await
        .unwrap()
        .value;
    let actions: Vec<&str> = entries.iter().map(|e| e.entry.action.as_str()).collect();
    assert_eq!(actions, ["maintenance.clear", "maintenance.set"]);
    assert!(entries.iter().all(|e| e.entry.actor == "test-admin"));
}

// ---- the resolver ----

#[tokio::test]
async fn the_resolver_says_whether_a_host_would_be_in_maintenance_and_why() {
    let app = TestApp::new().await;
    let (shop, _) = site(&app, "shop", "shop.example.com").await;
    site(&app, "blog", "blog.example.com").await;
    let (status, set) = put(
        &app,
        &format!("/api/subsystems/{shop}/maintenance"),
        json!({ "message": "shop only", "bypass": { "cidrs": ["10.0.0.0/8"] } }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{set}");

    let resolve = |host: &str, client: Option<&str>| {
        let mut body = json!({ "host": host, "path": "/x" });
        if let Some(client) = client {
            body["clientIp"] = json!(client);
        }
        let app = &app;
        async move { app.post("/api/resolve", body).await }
    };

    let (status, answered) = resolve("shop.example.com", Some("203.0.113.9")).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
    let maintenance = &answered["data"]["maintenance"];
    assert_eq!(maintenance["answeredBy"], "subsystem");
    assert_eq!(maintenance["scopes"][0]["outcome"], "respond");
    assert_eq!(maintenance["scopes"][0]["message"], "shop only");

    let (_, passed) = resolve("shop.example.com", Some("10.1.2.3")).await;
    assert!(passed["data"]["maintenance"]["answeredBy"].is_null());
    assert_eq!(
        passed["data"]["maintenance"]["scopes"][0]["outcome"],
        "bypassed"
    );

    let (_, untouched) = resolve("blog.example.com", None).await;
    assert!(untouched["data"]["maintenance"].is_null());

    let (status, bad) = resolve("shop.example.com", Some("not-an-ip")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(fields(&bad), ["clientIp"]);
}

#[tokio::test]
async fn the_resolver_lists_every_applying_scope_widest_first() {
    let app = TestApp::new().await;
    let (shop, api) = site(&app, "shop", "shop.example.com").await;
    put(&app, "/api/maintenance/global", json!({ "message": "g" })).await;
    put(
        &app,
        &format!("/api/subsystems/{shop}/maintenance"),
        json!({ "message": "s" }),
    )
    .await;
    put(
        &app,
        &format!("/api/subsystems/{shop}/routes/{api}/maintenance"),
        json!({ "message": "r" }),
    )
    .await;
    let (_, body) = app
        .post(
            "/api/resolve",
            json!({ "host": "shop.example.com", "path": "/api/x" }),
        )
        .await;
    let scopes: Vec<&str> = body["data"]["maintenance"]["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["scope"].as_str().unwrap())
        .collect();
    assert_eq!(scopes, ["global", "subsystem", "route"]);
    assert_eq!(body["data"]["maintenance"]["answeredBy"], "global");
}

#[tokio::test]
async fn a_rollback_does_not_change_maintenance_over_the_api() {
    let app = TestApp::new().await;
    site(&app, "shop", "shop.example.com").await;
    put(&app, "/api/maintenance/global", json!({ "message": "x" })).await;
    let (status, _) = app
        .call(Method::POST, "/api/revisions/1/rollback", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        app.get("/api/maintenance").await.1["data"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        app.config.list_maintenance().await.unwrap()[0].target,
        MaintenanceTarget::Global
    ));
}
