//! The limits API (M7, ADRs 0029 to 0031): rate-limit policies of subsystems and
//! routes, the route body cap and the group request cap, and `GET /api/limits`.
//! Each write is audited and live in the data plane's snapshot at once.

use axum::http::StatusCode;
use fast_proxy_model::Role;
use fast_proxy_routing::Lookup;
use serde_json::{json, Value};

use crate::test_support::TestApp;

const HOST: &str = "shop.example.com";

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

/// The rate limit the live snapshot applies to `path` on [`HOST`]: the key
/// header it uses, or `""` for the client address.
fn live_rate_limit(app: &TestApp, path: &str) -> Option<String> {
    let snapshot = app.state.config.snapshot_holder().load();
    match snapshot.lookup(HOST, path) {
        Lookup::Matched { route, .. } => route
            .route
            .rate_limit
            .as_ref()
            .map(|limit| limit.key_header().unwrap_or_default().to_string()),
        _ => panic!("{path} is routed"),
    }
}

async fn audited_actions(app: &TestApp) -> Vec<String> {
    use fast_proxy_store::repository::audit::{self, AuditQuery};
    app.config
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 20))
        .await
        .unwrap()
        .value
        .into_iter()
        .map(|e| format!("{} {}", e.entry.action, e.entry.target))
        .collect()
}

// ---- rate-limit policies ----

#[tokio::test]
async fn a_subsystem_policy_is_set_read_live_and_cleared() {
    let app = TestApp::new().await;
    let (shop, _) = app.site("shop", HOST).await;
    let uri = format!("/api/subsystems/{shop}/rate-limit");

    assert_eq!(app.get(&uri).await.0, StatusCode::NOT_FOUND, "none yet");
    let (status, set) = app.put(&uri, json!({ "rate": 10, "periodSecs": 60 })).await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(
        set["data"],
        json!({ "rate": 10, "periodSecs": 60, "burst": 10, "key": "client",
                "header": null, "ipv6Prefix": 64 }),
        "burst defaults to the rate, the key to the client"
    );
    let (_, read) = app.get(&uri).await;
    assert_eq!(read["data"], set["data"]);
    assert_eq!(live_rate_limit(&app, "/"), Some(String::new()));
    let (_, detail) = app.get(&format!("/api/subsystems/{shop}")).await;
    assert_eq!(detail["data"]["rateLimit"]["rate"], 10, "{detail}");

    let (status, cleared) = app.delete(&uri).await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert!(cleared["version"].as_u64().unwrap() > set["version"].as_u64().unwrap());
    assert_eq!(live_rate_limit(&app, "/"), None);
    assert_eq!(app.get(&uri).await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        app.delete(&uri).await.0,
        StatusCode::NOT_FOUND,
        "nothing left to clear"
    );

    assert_eq!(
        audited_actions(&app).await[..2],
        [
            format!("rate_limit.clear subsystem:{shop}"),
            format!("rate_limit.set subsystem:{shop}"),
        ]
    );
}

#[tokio::test]
async fn a_route_policy_replaces_its_subsystems_for_that_route() {
    let app = TestApp::new().await;
    let (shop, root) = app.site("shop", HOST).await;
    let upstream = app
        .get(&format!("/api/subsystems/{shop}/upstreams"))
        .await
        .1["data"][0]["id"]
        .clone();
    let (_, api) = app
        .post(
            &format!("/api/subsystems/{shop}/routes"),
            json!({ "prefix": "/api", "upstreamGroupId": upstream }),
        )
        .await;
    let api = api["data"]["id"].as_i64().unwrap();
    app.put(
        &format!("/api/subsystems/{shop}/rate-limit"),
        json!({ "rate": 100, "periodSecs": 1 }),
    )
    .await;
    let route_uri = format!("/api/subsystems/{shop}/routes/{api}/rate-limit");
    let (status, set) = app
        .put(
            &route_uri,
            json!({ "rate": 5, "periodSecs": 60, "burst": 2, "key": "header",
                    "header": "X-Customer" }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{set}");
    assert_eq!(set["data"]["header"], "x-customer");

    assert_eq!(
        live_rate_limit(&app, "/api/orders"),
        Some("x-customer".into())
    );
    assert_eq!(
        live_rate_limit(&app, "/"),
        Some(String::new()),
        "the other routes keep the subsystem's"
    );
    let (_, routes) = app.get(&format!("/api/subsystems/{shop}")).await;
    let shown: Vec<&Value> = routes["data"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| &route["rateLimit"])
        .collect();
    assert!(shown.contains(&&set["data"]), "{routes}");
    assert_eq!(
        app.get(&format!("/api/subsystems/{shop}/routes/{root}/rate-limit"))
            .await
            .0,
        StatusCode::NOT_FOUND,
        "a route without its own policy has none to read"
    );

    assert_eq!(app.delete(&route_uri).await.0, StatusCode::OK);
    assert_eq!(live_rate_limit(&app, "/api/orders"), Some(String::new()));
    assert_eq!(
        audited_actions(&app).await[0],
        format!("rate_limit.clear subsystem:{shop}/route:{api}")
    );
}

#[tokio::test]
async fn a_bad_policy_names_each_field_and_changes_nothing() {
    let app = TestApp::new().await;
    let (shop, _) = app.site("shop", HOST).await;
    let uri = format!("/api/subsystems/{shop}/rate-limit");
    let cases = [
        (json!({ "rate": 0, "periodSecs": 60 }), "rateLimit.rate"),
        (
            json!({ "rate": 1, "periodSecs": 0 }),
            "rateLimit.periodSecs",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "burst": 0 }),
            "rateLimit.burst",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "ipv6Prefix": 8 }),
            "rateLimit.ipv6Prefix",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "key": "cookie" }),
            "rateLimit.key",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "key": "header" }),
            "rateLimit.header",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "header": "x-customer" }),
            "rateLimit.header",
        ),
        (
            json!({ "rate": 1, "periodSecs": 1, "key": "header", "header": "bad header" }),
            "rateLimit.header",
        ),
    ];
    for (body, field) in cases {
        let (status, refused) = app.put(&uri, body.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {refused}");
        assert_eq!(fields(&refused), [field], "{body}");
    }
    let (status, _) = app
        .put(&uri, json!({ "rate": 1, "periodSecs": 1, "extra": true }))
        .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "unknown fields are refused"
    );
    assert_eq!(live_rate_limit(&app, "/"), None);
}

#[tokio::test]
async fn an_unknown_subsystem_or_route_is_404() {
    let app = TestApp::new().await;
    let (shop, _) = app.site("shop", HOST).await;
    let policy = json!({ "rate": 1, "periodSecs": 1 });
    for uri in [
        "/api/subsystems/999/rate-limit".to_string(),
        format!("/api/subsystems/{shop}/routes/999/rate-limit"),
    ] {
        assert_eq!(app.get(&uri).await.0, StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(
            app.put(&uri, policy.clone()).await.0,
            StatusCode::NOT_FOUND,
            "{uri}"
        );
        assert_eq!(app.delete(&uri).await.0, StatusCode::NOT_FOUND, "{uri}");
    }
}

#[tokio::test]
async fn a_viewer_reads_policies_and_limits_but_changes_nothing() {
    let mut app = TestApp::new().await;
    let (shop, _) = app.site("shop", HOST).await;
    let uri = format!("/api/subsystems/{shop}/rate-limit");
    app.put(&uri, json!({ "rate": 1, "periodSecs": 1 })).await;
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);

    assert_eq!(app.get(&uri).await.0, StatusCode::OK);
    assert_eq!(app.get("/api/limits").await.0, StatusCode::OK);
    assert_eq!(
        app.put(&uri, json!({ "rate": 2, "periodSecs": 1 })).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.delete(&uri).await.0, StatusCode::FORBIDDEN);
    assert_eq!(live_rate_limit(&app, "/"), Some(String::new()));
}

// ---- the status of the limits ----

#[tokio::test]
async fn the_limits_status_shows_the_settings_the_key_table_and_the_refusals() {
    let app = TestApp::new().await;
    app.metrics.request_refused("rate_limited");
    app.metrics.request_refused("rate_limited");
    app.metrics.request_refused("body_too_large");
    app.metrics.connection_refused("per_client");
    app.metrics.client_timeout("first_byte");
    app.metrics.rate_limit_table_full();

    let (status, body) = app.get("/api/limits").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let data = &body["data"];
    assert_eq!(data["settings"]["maxConnections"], 10_000, "{body}");
    assert_eq!(data["settings"]["maxConnectionsPerClient"], 256);
    assert_eq!(data["settings"]["tunnelIdleTimeoutMs"], 3_600_000);
    assert_eq!(data["settings"]["maxBodyBytes"], Value::Null);
    assert_eq!(data["rateLimitKeys"]["used"], 0);
    assert!(data["rateLimitKeys"]["capacity"].as_u64().unwrap() > 0);
    assert!(data["requestsRefused"]
        .as_array()
        .unwrap()
        .contains(&json!({ "reason": "rate_limited", "count": 2 })));
    assert!(data["requestsRefused"]
        .as_array()
        .unwrap()
        .contains(&json!({ "reason": "body_too_large", "count": 1 })));
    assert_eq!(
        data["connectionsRefused"],
        json!([{ "reason": "per_client", "count": 1 }])
    );
    assert_eq!(
        data["clientTimeouts"],
        json!([{ "reason": "first_byte", "count": 1 }])
    );
    assert_eq!(data["rateLimitTableFull"], 1);
}

// ---- the route body cap and the group request cap ----

/// The `maxRequests` of group `id` as the list of groups shows it.
async fn listed_cap(app: &TestApp, shop: i64, id: i64) -> Value {
    let (_, groups) = app.get(&format!("/api/subsystems/{shop}/upstreams")).await;
    groups["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|group| group["id"] == id)
        .unwrap()["maxRequests"]
        .clone()
}

#[tokio::test]
async fn a_routes_body_cap_round_trips_is_live_and_is_validated() {
    let app = TestApp::new().await;
    let (shop, root) = app.site("shop", HOST).await;
    let uri = format!("/api/subsystems/{shop}/routes/{root}");
    let (_, current) = app.get(&format!("/api/subsystems/{shop}/routes")).await;
    let existing = &current["data"][0];
    let route = json!({
        "prefix": existing["prefix"],
        "upstreamGroupId": existing["upstreamGroupId"],
        "maxBodyBytes": 1024,
    });
    let (status, updated) = app.put(&uri, route.clone()).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["data"]["maxBodyBytes"], 1024);
    let snapshot = app.state.config.snapshot_holder().load();
    let Lookup::Matched { route: live, .. } = snapshot.lookup(HOST, "/") else {
        panic!("routed");
    };
    assert_eq!(live.route.max_body_bytes, Some(1024));

    for bad in [0u64, (1 << 40) + 1] {
        let mut body = route.clone();
        body["maxBodyBytes"] = json!(bad);
        let (status, refused) = app.put(&uri, body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(fields(&refused), ["maxBodyBytes"]);
    }

    let mut without = route.clone();
    without.as_object_mut().unwrap().remove("maxBodyBytes");
    let (_, updated) = app.put(&uri, without).await;
    assert_eq!(
        updated["data"]["maxBodyBytes"],
        Value::Null,
        "an update without the cap removes it"
    );
}

#[tokio::test]
async fn a_groups_request_cap_round_trips_is_live_and_is_validated() {
    let app = TestApp::new().await;
    let (shop, _) = app.site("shop", HOST).await;
    let (status, created) = app
        .post(
            &format!("/api/subsystems/{shop}/upstreams"),
            json!({ "name": "capped", "endpoints": [{ "addr": "127.0.0.1:3001" }],
                    "maxRequests": 50 }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["data"]["maxRequests"], 50);
    let id = created["data"]["id"].as_i64().unwrap();
    let uri = format!("/api/subsystems/{shop}/upstreams/{id}");
    assert_eq!(listed_cap(&app, shop, id).await, 50);

    for bad in [0, u32::MAX] {
        let (status, refused) = app
            .put(&uri, json!({ "name": "capped", "maxRequests": bad }))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(fields(&refused), ["maxRequests"]);
    }

    let (status, updated) = app.put(&uri, json!({ "name": "capped", })).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(
        updated["data"]["maxRequests"],
        Value::Null,
        "an update without the cap removes it"
    );
    assert_eq!(listed_cap(&app, shop, id).await, Value::Null);
}
