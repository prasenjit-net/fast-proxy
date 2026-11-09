//! HTTP-level tests of the subsystem, host, upstream, and route endpoints:
//! success, validation errors with `fields`, 404, and 409, plus proof that
//! every successful write is visible to the data plane without a restart.

use axum::http::{Method, StatusCode};
use fast_proxy_routing::Lookup;
use serde_json::{json, Value};

use crate::test_support::TestApp;

/// Creates a subsystem with a host, an upstream, and a `/` route; returns
/// the subsystem id and the upstream id.
async fn create_site(app: &TestApp, name: &str, host: &str, addr: &str) -> (i64, i64) {
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
            json!({ "name": "backend", "addr": addr }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let upstream = body["data"]["id"].as_i64().unwrap();
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/routes"),
            json!({ "prefix": "/", "upstreamGroupId": upstream }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    (id, upstream)
}

fn error_fields(body: &Value) -> Vec<String> {
    body["error"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["field"].as_str().unwrap().to_string())
        .collect()
}

fn backend_for(app: &TestApp, host: &str, path: &str) -> Option<String> {
    let snapshot = app.config.snapshot_holder().load();
    match snapshot.lookup(host, path) {
        Lookup::Matched { route, .. } => Some(route.route.endpoint.to_string()),
        _ => None,
    }
}

// ---- subsystems ---------------------------------------------------------

#[tokio::test]
async fn the_list_starts_empty_at_version_zero() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/subsystems").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"], json!([]));
    assert_eq!(body["version"], 0);
}

#[tokio::test]
async fn creating_a_subsystem_returns_it_with_defaults_and_the_new_version() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post(
            "/api/subsystems",
            json!({ "name": "shop", "description": "the shop" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["version"], 1);
    assert_eq!(body["data"]["name"], "shop");
    assert_eq!(body["data"]["enabled"], true);
    assert_eq!(body["data"]["description"], "the shop");
    assert!(body["data"]["updatedAtMs"].as_i64().unwrap() > 0);
    assert_eq!(body["data"]["hosts"], json!([]));
}

#[tokio::test]
async fn the_list_shows_host_patterns_and_route_counts() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (_, body) = app.get("/api/subsystems").await;
    let shop = &body["data"][0];
    assert_eq!(shop["name"], "shop");
    assert_eq!(shop["hosts"], json!(["shop.example.com"]));
    assert_eq!(shop["routeCount"], 1);
    assert_eq!(body["version"], 4);
}

#[tokio::test]
async fn a_duplicate_subsystem_name_is_a_409() {
    let app = TestApp::new().await;
    app.post("/api/subsystems", json!({ "name": "shop" })).await;
    let (status, body) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "CONFLICT");
}

#[tokio::test]
async fn an_invalid_subsystem_is_a_400_naming_every_bad_field() {
    let app = TestApp::new().await;
    let long = "x".repeat(501);
    let (status, body) = app
        .post(
            "/api/subsystems",
            json!({ "name": "Bad Name", "description": long }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "VALIDATION_FAILED");
    assert_eq!(error_fields(&body), ["name", "description"]);
}

#[tokio::test]
async fn malformed_bodies_get_the_json_error_envelope() {
    let app = TestApp::new().await;
    let (status, body) = app
        .call_raw(Method::POST, "/api/subsystems", Some("{not json"))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "BAD_REQUEST");

    let (status, body) = app
        .post("/api/subsystems", json!({ "name": "shop", "nmae": "typo" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "BAD_REQUEST");
}

#[tokio::test]
async fn getting_updating_and_deleting_a_subsystem() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;

    let (status, body) = app.get(&format!("/api/subsystems/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["hosts"][0]["pattern"], "shop.example.com");
    assert_eq!(body["data"]["upstreams"][0]["addr"], "127.0.0.1:3000");
    assert_eq!(body["data"]["routes"][0]["prefix"], "/");

    let (status, body) = app
        .put(
            &format!("/api/subsystems/{id}"),
            json!({ "name": "store", "enabled": false, "description": "renamed" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["name"], "store");
    assert_eq!(body["data"]["enabled"], false);
    assert_eq!(body["version"], 5);

    let (status, body) = app.delete(&format!("/api/subsystems/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], 6);
    let (status, _) = app.get(&format!("/api/subsystems/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn missing_subsystems_are_404_everywhere() {
    let app = TestApp::new().await;
    let upstream = json!({ "name": "b", "addr": "127.0.0.1:1" });
    let route = json!({ "prefix": "/", "upstreamGroupId": 1 });
    for (method, uri, body) in [
        (Method::GET, "/api/subsystems/9", None),
        (
            Method::PUT,
            "/api/subsystems/9",
            Some(json!({ "name": "x" })),
        ),
        (Method::DELETE, "/api/subsystems/9", None),
        (
            Method::POST,
            "/api/subsystems/9/hosts",
            Some(json!({ "pattern": "a.com" })),
        ),
        (Method::DELETE, "/api/subsystems/9/hosts/1", None),
        (Method::GET, "/api/subsystems/9/upstreams", None),
        (
            Method::POST,
            "/api/subsystems/9/upstreams",
            Some(upstream.clone()),
        ),
        (Method::PUT, "/api/subsystems/9/upstreams/1", Some(upstream)),
        (Method::DELETE, "/api/subsystems/9/upstreams/1", None),
        (Method::GET, "/api/subsystems/9/routes", None),
        (
            Method::POST,
            "/api/subsystems/9/routes",
            Some(route.clone()),
        ),
        (Method::PUT, "/api/subsystems/9/routes/1", Some(route)),
        (Method::DELETE, "/api/subsystems/9/routes/1", None),
    ] {
        let (status, _) = app.call(method.clone(), uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
    assert_eq!(app.config.snapshot_holder().load().version, 0);
}

// ---- hosts --------------------------------------------------------------

#[tokio::test]
async fn adding_and_removing_a_host() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/hosts"),
            json!({ "pattern": "*.Example.org" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["pattern"], "*.example.org");
    let host_id = body["data"]["id"].as_i64().unwrap();

    let (status, _) = app
        .delete(&format!("/api/subsystems/{id}/hosts/{host_id}"))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .delete(&format!("/api/subsystems/{id}/hosts/{host_id}"))
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_host_pattern_cannot_be_shared_between_subsystems() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (blog, _) = create_site(&app, "blog", "blog.example.com", "127.0.0.1:3001").await;
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{blog}/hosts"),
            json!({ "pattern": "shop.example.com" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn an_invalid_host_is_reported_under_the_pattern_field() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/hosts"),
            json!({ "pattern": "bad host:80" }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["pattern"]);
}

// ---- upstreams ----------------------------------------------------------

#[tokio::test]
async fn upstream_crud() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let base = format!("/api/subsystems/{id}/upstreams");

    let (status, body) = app
        .post(
            &base,
            json!({ "name": "api", "addr": "API.internal:8080", "weight": 3 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["addr"], "api.internal:8080");
    assert_eq!(body["data"]["weight"], 3);
    let api = body["data"]["id"].as_i64().unwrap();

    let (_, body) = app.get(&base).await;
    assert_eq!(body["data"].as_array().unwrap().len(), 2);

    let (status, body) = app
        .put(
            &format!("{base}/{api}"),
            json!({ "name": "api2", "addr": "127.0.0.1:9090" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["name"], "api2");
    assert_eq!(body["data"]["weight"], 1);

    let (status, _) = app.delete(&format!("{base}/{api}")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app.delete(&format!("{base}/{api}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_upstream_with_several_bad_fields_reports_them_all() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({ "name": "  ", "addr": "no-port", "weight": 0 }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["name", "addr", "weight"]);
}

#[tokio::test]
async fn a_duplicate_upstream_name_is_a_409() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/upstreams"),
            json!({ "name": "backend", "addr": "127.0.0.1:1" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn deleting_an_upstream_a_route_uses_is_a_409() {
    let app = TestApp::new().await;
    let (id, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app
        .delete(&format!("/api/subsystems/{id}/upstreams/{upstream}"))
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("used by 1 route"));
}

// ---- routes -------------------------------------------------------------

#[tokio::test]
async fn route_crud_with_defaults_and_match_order() {
    let app = TestApp::new().await;
    let (id, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let base = format!("/api/subsystems/{id}/routes");

    let (status, body) = app
        .post(
            &base,
            json!({ "prefix": "/api/", "upstreamGroupId": upstream }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["prefix"], "/api");
    assert_eq!(body["data"]["stripPrefix"], false);
    assert_eq!(body["data"]["preserveHost"], true);
    assert_eq!(body["data"]["connectTimeoutMs"], 5000);
    assert_eq!(body["data"]["responseTimeoutMs"], 30000);
    assert_eq!(body["data"]["enabled"], true);
    let api = body["data"]["id"].as_i64().unwrap();

    app.post(
        &base,
        json!({ "prefix": "/api/v1", "upstreamGroupId": upstream }),
    )
    .await;
    let (_, body) = app.get(&base).await;
    let order: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["prefix"].as_str().unwrap())
        .collect();
    assert_eq!(order, ["/api/v1", "/api", "/"], "longest prefix first");

    let (status, body) = app
        .put(
            &format!("{base}/{api}"),
            json!({
                "prefix": "/shop", "upstreamGroupId": upstream, "stripPrefix": true,
                "upstreamPrefix": "/v2", "preserveHost": false,
                "connectTimeoutMs": 1000, "responseTimeoutMs": 2000, "enabled": false
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["upstreamPrefix"], "/v2");
    assert_eq!(body["data"]["stripPrefix"], true);
    assert_eq!(body["data"]["enabled"], false);

    let (status, _) = app.delete(&format!("{base}/{api}")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app.delete(&format!("{base}/{api}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_route_with_bad_fields_reports_them_camel_cased() {
    let app = TestApp::new().await;
    let (id, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{id}/routes"),
            json!({
                "prefix": "no-slash", "upstreamGroupId": upstream, "upstreamPrefix": "/a/../b",
                "connectTimeoutMs": 5, "responseTimeoutMs": 99999999
            }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error_fields(&body),
        [
            "prefix",
            "upstreamPrefix",
            "connectTimeoutMs",
            "responseTimeoutMs"
        ]
    );
}

#[tokio::test]
async fn a_duplicate_route_prefix_is_a_409() {
    let app = TestApp::new().await;
    let (id, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/routes"),
            json!({ "prefix": "/", "upstreamGroupId": upstream }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_route_must_use_an_upstream_of_its_own_subsystem() {
    let app = TestApp::new().await;
    let (shop, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (_, blog_upstream) = create_site(&app, "blog", "blog.example.com", "127.0.0.1:3001").await;
    let (status, body) = app
        .post(
            &format!("/api/subsystems/{shop}/routes"),
            json!({ "prefix": "/x", "upstreamGroupId": blog_upstream }),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["upstreamGroupId"]);
}

// ---- live effect on the data plane --------------------------------------

#[tokio::test]
async fn every_write_is_visible_to_the_data_plane_without_a_restart() {
    let app = TestApp::new().await;
    assert_eq!(backend_for(&app, "shop.example.com", "/"), None);

    let (shop, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    assert_eq!(
        backend_for(&app, "shop.example.com", "/").as_deref(),
        Some("127.0.0.1:3000")
    );

    // Repoint the upstream: the very next lookup sees the new backend.
    app.put(
        &format!("/api/subsystems/{shop}/upstreams/{upstream}"),
        json!({ "name": "backend", "addr": "127.0.0.1:4000" }),
    )
    .await;
    assert_eq!(
        backend_for(&app, "shop.example.com", "/").as_deref(),
        Some("127.0.0.1:4000")
    );

    // A new route for a prefix.
    app.post(
        &format!("/api/subsystems/{shop}/upstreams"),
        json!({ "name": "api", "addr": "127.0.0.1:5000" }),
    )
    .await;
    let (_, upstreams) = app.get(&format!("/api/subsystems/{shop}/upstreams")).await;
    let api = upstreams["data"][1]["id"].as_i64().unwrap();
    app.post(
        &format!("/api/subsystems/{shop}/routes"),
        json!({ "prefix": "/api", "upstreamGroupId": api }),
    )
    .await;
    assert_eq!(
        backend_for(&app, "shop.example.com", "/api/x").as_deref(),
        Some("127.0.0.1:5000")
    );
    assert_eq!(
        backend_for(&app, "shop.example.com", "/other").as_deref(),
        Some("127.0.0.1:4000")
    );

    // A second host, then disabling the subsystem, then deleting it.
    app.post(
        &format!("/api/subsystems/{shop}/hosts"),
        json!({ "pattern": "*.shop.test" }),
    )
    .await;
    assert!(backend_for(&app, "a.shop.test", "/").is_some());
    app.put(
        &format!("/api/subsystems/{shop}"),
        json!({ "name": "shop", "enabled": false }),
    )
    .await;
    assert_eq!(backend_for(&app, "shop.example.com", "/"), None);
    app.delete(&format!("/api/subsystems/{shop}")).await;
    assert_eq!(backend_for(&app, "a.shop.test", "/"), None);
}

#[tokio::test]
async fn a_rejected_write_changes_neither_the_version_nor_the_data_plane() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let before = app.config.snapshot_holder().load().version;
    let (status, _) = app.post("/api/subsystems", json!({ "name": "shop" })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(app.config.snapshot_holder().load().version, before);
    assert_eq!(
        backend_for(&app, "shop.example.com", "/").as_deref(),
        Some("127.0.0.1:3000")
    );
}

// ---- revisions ----------------------------------------------------------

#[tokio::test]
async fn revisions_list_newest_first_with_their_summaries() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, body) = app.get("/api/revisions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["version"], 4);
    let summaries: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["summary"].as_str().unwrap())
        .collect();
    assert_eq!(
        summaries,
        [
            "route created: / in shop",
            "upstream created: backend in shop",
            "host added: shop.example.com in shop",
            "subsystem created: shop",
        ]
    );
    assert_eq!(body["data"][0]["version"], 4);
    assert!(body["data"][0]["createdAtMs"].as_i64().unwrap() > 0);
    assert!(body["nextBefore"].is_null());
}

#[tokio::test]
async fn revisions_are_paged_with_limit_and_before() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;

    let (_, first) = app.get("/api/revisions?limit=3").await;
    let versions: Vec<i64> = first["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["version"].as_i64().unwrap())
        .collect();
    assert_eq!(versions, [4, 3, 2]);
    assert_eq!(first["nextBefore"], 2);

    let (_, second) = app.get("/api/revisions?limit=3&before=2").await;
    assert_eq!(second["data"].as_array().unwrap().len(), 1);
    assert_eq!(second["data"][0]["version"], 1);
    assert!(second["nextBefore"].is_null());
}

#[tokio::test]
async fn bad_paging_parameters_are_reported_by_name() {
    let app = TestApp::new().await;
    let (status, body) = app.get("/api/revisions?limit=0&before=abc").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["limit", "before"]);
}

#[tokio::test]
async fn a_revision_shows_the_whole_configuration_as_of_then() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    app.delete(&format!("/api/subsystems/{id}")).await;

    let (status, body) = app.get("/api/revisions/4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["summary"], "route created: / in shop");
    let shop = &body["data"]["config"]["subsystems"][0];
    assert_eq!(shop["name"], "shop");
    assert_eq!(shop["hosts"], json!(["shop.example.com"]));
    assert_eq!(shop["upstreams"][0]["addr"], "127.0.0.1:3000");
    assert_eq!(shop["routes"][0]["prefix"], "/");
    assert_eq!(body["version"], 5);

    let (_, after_delete) = app.get("/api/revisions/5").await;
    assert_eq!(after_delete["data"]["config"]["subsystems"], json!([]));

    let (status, body) = app.get("/api/revisions/99").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "NOT_FOUND");
}

#[tokio::test]
async fn rolling_back_restores_the_earlier_behavior_as_a_new_revision() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    app.delete(&format!("/api/subsystems/{id}")).await;
    assert_eq!(backend_for(&app, "shop.example.com", "/"), None);

    let (status, body) = app.post("/api/revisions/4/rollback", json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["restoredFrom"], 4);
    assert_eq!(body["version"], 6);
    assert_eq!(
        backend_for(&app, "shop.example.com", "/").as_deref(),
        Some("127.0.0.1:3000")
    );

    // The restored subsystem is the same one, with the same ids.
    let (status, restored) = app.get(&format!("/api/subsystems/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restored["data"]["name"], "shop");

    let (_, revisions) = app.get("/api/revisions?limit=1").await;
    assert_eq!(revisions["data"][0]["summary"], "rollback to v4");
    assert_eq!(revisions["data"][0]["version"], 6);
}

#[tokio::test]
async fn rolling_back_to_a_missing_revision_is_a_404_and_changes_nothing() {
    let app = TestApp::new().await;
    create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (status, _) = app.post("/api/revisions/99/rollback", json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(app.config.snapshot_holder().load().version, 4);
}

// ---- resolver -----------------------------------------------------------

async fn resolve(app: &TestApp, host: &str, path: &str) -> Value {
    let (status, body) = app
        .post("/api/resolve", json!({ "host": host, "path": path }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// A shop with a catch-all route and a rewriting `/api` route.
async fn shop_with_rewrite(app: &TestApp) -> i64 {
    let (id, _) = create_site(app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    let (_, upstreams) = app.get(&format!("/api/subsystems/{id}/upstreams")).await;
    let upstream = upstreams["data"][0]["id"].as_i64().unwrap();
    let (status, _) = app
        .post(
            &format!("/api/subsystems/{id}/routes"),
            json!({
                "prefix": "/api", "upstreamGroupId": upstream, "stripPrefix": true,
                "upstreamPrefix": "/v2", "preserveHost": false
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    id
}

#[tokio::test]
async fn the_resolver_reports_the_exact_upstream_url() {
    let app = TestApp::new().await;
    shop_with_rewrite(&app).await;

    let body = resolve(&app, "shop.example.com", "/api/users?page=2&q=a%20b").await;
    let result = &body["data"];
    assert_eq!(result["matched"], true);
    assert!(result["reason"].is_null());
    assert_eq!(result["subsystem"]["name"], "shop");
    assert_eq!(result["route"]["prefix"], "/api");
    assert_eq!(result["route"]["stripPrefix"], true);
    assert_eq!(result["route"]["upstreamPrefix"], "/v2");
    assert_eq!(result["route"]["preserveHost"], false);
    assert_eq!(result["route"]["endpoint"], "127.0.0.1:3000");
    assert_eq!(
        result["upstreamUrl"],
        "http://127.0.0.1:3000/v2/users?page=2&q=a%20b"
    );
    assert_eq!(body["version"], app.config.snapshot_holder().load().version);

    let root = resolve(&app, "shop.example.com", "/other").await;
    assert_eq!(root["data"]["route"]["prefix"], "/");
    assert_eq!(root["data"]["upstreamUrl"], "http://127.0.0.1:3000/other");
}

#[tokio::test]
async fn the_resolver_normalizes_the_host_like_the_data_plane() {
    let app = TestApp::new().await;
    shop_with_rewrite(&app).await;
    let body = resolve(&app, "SHOP.Example.com:8000", "/").await;
    assert_eq!(body["data"]["matched"], true);
}

#[tokio::test]
async fn the_resolver_explains_why_nothing_matched() {
    let app = TestApp::new().await;
    let id = shop_with_rewrite(&app).await;

    let unknown = resolve(&app, "nope.example.com", "/").await;
    assert_eq!(unknown["data"]["matched"], false);
    assert_eq!(unknown["data"]["reason"], "unknown_host");
    assert!(unknown["data"]["subsystem"].is_null());
    assert!(unknown["data"]["upstreamUrl"].is_null());

    // Remove the catch-all so only /api remains.
    let (_, routes) = app.get(&format!("/api/subsystems/{id}/routes")).await;
    let root = routes["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["prefix"] == "/")
        .unwrap()["id"]
        .as_i64()
        .unwrap();
    app.delete(&format!("/api/subsystems/{id}/routes/{root}"))
        .await;
    let no_route = resolve(&app, "shop.example.com", "/other").await;
    assert_eq!(no_route["data"]["reason"], "no_route");
    assert_eq!(no_route["data"]["subsystem"]["name"], "shop");

    app.put(
        &format!("/api/subsystems/{id}"),
        json!({ "name": "shop", "enabled": false }),
    )
    .await;
    let disabled = resolve(&app, "shop.example.com", "/api/x").await;
    assert_eq!(disabled["data"]["reason"], "subsystem_disabled");
    assert_eq!(disabled["data"]["subsystem"]["name"], "shop");
}

#[tokio::test]
async fn the_resolver_follows_live_edits() {
    let app = TestApp::new().await;
    let (id, upstream) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    app.put(
        &format!("/api/subsystems/{id}/upstreams/{upstream}"),
        json!({ "name": "backend", "addr": "127.0.0.1:4000" }),
    )
    .await;
    let body = resolve(&app, "shop.example.com", "/x").await;
    assert_eq!(body["data"]["upstreamUrl"], "http://127.0.0.1:4000/x");
}

#[tokio::test]
async fn the_resolver_validates_its_input() {
    let app = TestApp::new().await;
    let (status, body) = app
        .post("/api/resolve", json!({ "host": " ", "path": "no-slash" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["host", "path"]);

    let (status, body) = app
        .post("/api/resolve", json!({ "host": "a.com", "path": "/a b" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error_fields(&body), ["path"]);

    let (status, _) = app.post("/api/resolve", json!({ "host": "a.com" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ---- status -------------------------------------------------------------

#[tokio::test]
async fn status_reports_what_is_running() {
    let app = TestApp::new().await;
    let (id, _) = create_site(&app, "shop", "shop.example.com", "127.0.0.1:3000").await;
    create_site(&app, "blog", "blog.example.com", "127.0.0.1:3001").await;
    app.put(
        &format!("/api/subsystems/{id}"),
        json!({ "name": "shop", "enabled": false }),
    )
    .await;

    let (status, body) = app.get("/api/status").await;
    assert_eq!(status, StatusCode::OK);
    let data = &body["data"];
    assert_eq!(body["version"], 9);
    assert_eq!(
        data["listeners"],
        json!([{ "name": "http", "addr": "127.0.0.1:8000" }])
    );
    assert_eq!(data["database"], "/tmp/test.db");
    assert_eq!(data["subsystems"], json!({ "total": 2, "enabled": 1 }));
    assert!(data["startedAtMs"].as_i64().unwrap() > 0);
    assert!(data["uptimeSecs"].is_u64());
}
