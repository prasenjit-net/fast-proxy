//! The samples API: the list and its filters, one sample, the export, clearing,
//! who may do any of it, and what is audited.

use axum::http::{Method, StatusCode};
use fast_proxy_model::{Role, Trigger};
use fast_proxy_sampling::{Sample, SampleRequest, SampleResponse, SampleUpstream};
use fast_proxy_store::repository::audit::{self, AuditQuery};
use serde_json::Value;

use crate::events::Event;
use crate::test_support::TestApp;

fn sample(n: i64, instance: &str, subsystem: &str) -> Sample {
    Sample {
        id: None,
        at_ms: n,
        instance: instance.into(),
        request_id: format!("req-{instance}-{n}"),
        triggers: vec![Trigger::UpstreamFailure.as_str().into()],
        primary: Trigger::UpstreamFailure.as_str().into(),
        subsystem: subsystem.into(),
        route: "/api".into(),
        client: "203.0.113.9:5000".into(),
        request: SampleRequest {
            method: "GET".into(),
            path: "/api/orders".into(),
            query: Some("page=2".into()),
            headers: vec![("accept".into(), "*/*".into())],
            bytes: 0,
            body: None,
            unusual: Vec::new(),
        },
        response: SampleResponse {
            status: 502,
            headers: Vec::new(),
            bytes: 0,
            body: None,
            body_omitted: Some("off".into()),
        },
        total_ms: 30,
        upstream: SampleUpstream {
            attempts: Vec::new(),
        },
        suppressed: None,
    }
}

/// Five samples from two instances, with something different about each.
fn stored(app: &TestApp) {
    let mut slow = sample(3, "a", "shop");
    slow.triggers = vec!["slow".into()];
    slow.primary = "slow".into();
    slow.total_ms = 5000;
    slow.response.status = 200;
    let mut other = sample(4, "b", "blog");
    other.route = "/static".into();
    other.client = "198.51.100.1:7".into();
    other.response.status = 404;
    app.store_samples(vec![sample(1, "a", "shop"), sample(2, "a", "shop"), slow]);
    app.store_samples(vec![other, sample(5, "b", "shop")]);
}

fn ids(page: &Value) -> Vec<String> {
    page["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["requestId"].as_str().unwrap().to_string())
        .collect()
}

async fn audit_actions(app: &TestApp) -> Vec<String> {
    app.config
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 100))
        .await
        .unwrap()
        .value
        .into_iter()
        .map(|row| row.entry.action)
        .collect()
}

#[tokio::test]
async fn the_list_shows_every_instances_samples_newest_first_without_their_detail() {
    let app = TestApp::new().await;
    stored(&app);
    let (status, page) = app.get("/api/samples").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        ids(&page),
        ["req-b-5", "req-b-4", "req-a-3", "req-a-2", "req-a-1"]
    );
    let first = &page["data"][0];
    assert_eq!(first["instance"], "b");
    assert_eq!(first["primary"], "upstream_failure");
    assert_eq!(first["triggers"][0], "upstream_failure");
    assert_eq!(first["status"], 502);
    assert_eq!(first["totalMs"], 30);
    assert!(first["sizeBytes"].as_u64().unwrap() > 0);
    assert!(
        first.get("request").is_none(),
        "no headers or bodies in a list"
    );
    assert_eq!(page["nextBefore"], Value::Null);
}

#[tokio::test]
async fn every_filter_selects_what_it_names() {
    let app = TestApp::new().await;
    stored(&app);
    for (query, expected) in [
        ("trigger=slow", vec!["req-a-3"]),
        ("trigger=slow,retried", vec!["req-a-3"]),
        ("statusClass=4", vec!["req-b-4"]),
        ("subsystem=blog", vec!["req-b-4"]),
        ("route=/static", vec!["req-b-4"]),
        ("instance=a", vec!["req-a-3", "req-a-2", "req-a-1"]),
        ("client=198.51.100.1:7", vec!["req-b-4"]),
        ("minMs=1000", vec!["req-a-3"]),
        ("since=4", vec!["req-b-5", "req-b-4"]),
        ("subsystem=shop&instance=b", vec!["req-b-5"]),
    ] {
        let (status, page) = app.get(&format!("/api/samples?{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {page}");
        assert_eq!(ids(&page), expected, "{query}");
    }
}

#[tokio::test]
async fn pages_are_followed_with_before_until_the_last() {
    let app = TestApp::new().await;
    stored(&app);
    let (_, first) = app.get("/api/samples?limit=2").await;
    assert_eq!(ids(&first), ["req-b-5", "req-b-4"]);
    let next = first["nextBefore"].as_i64().unwrap();
    let (_, second) = app
        .get(&format!("/api/samples?limit=2&before={next}"))
        .await;
    assert_eq!(ids(&second), ["req-a-3", "req-a-2"]);
    let next = second["nextBefore"].as_i64().unwrap();
    let (_, third) = app
        .get(&format!("/api/samples?limit=2&before={next}"))
        .await;
    assert_eq!(ids(&third), ["req-a-1"]);
    assert_eq!(third["nextBefore"], Value::Null, "a short page is the last");
}

#[tokio::test]
async fn a_bad_filter_is_a_400_naming_the_field() {
    let app = TestApp::new().await;
    for (query, field) in [
        ("trigger=server_error", "trigger"),
        ("statusClass=9", "statusClass"),
        ("statusClass=x", "statusClass"),
        ("limit=abc", "limit"),
        ("before=-1", "before"),
        ("minMs=slow", "minMs"),
    ] {
        let (status, body) = app.get(&format!("/api/samples?{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
        let fields = body["error"]["fields"].as_array().unwrap();
        assert!(
            fields.iter().any(|f| f["field"] == field),
            "{query}: {body}"
        );
    }
}

#[tokio::test]
async fn one_sample_comes_back_whole_and_a_missing_one_is_a_404() {
    let app = TestApp::new().await;
    stored(&app);
    let (_, page) = app.get("/api/samples").await;
    let id = page["data"][0]["id"].as_i64().unwrap();
    let (status, body) = app.get(&format!("/api/samples/{id}")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let sample = &body["data"];
    assert_eq!(sample["id"], id);
    assert_eq!(sample["requestId"], "req-b-5");
    assert_eq!(sample["request"]["query"], "page=2");
    assert_eq!(sample["request"]["headers"][0][0], "accept");
    assert_eq!(sample["response"]["bodyOmitted"], "off");
    assert_eq!(
        app.get("/api/samples/999999").await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_export_is_ndjson_newest_first_follows_the_filters_and_is_audited() {
    let app = TestApp::new().await;
    stored(&app);
    let (status, content_type, text) = app.get_text("/api/samples/export?instance=a").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/x-ndjson");
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let request_ids: Vec<&str> = lines
        .iter()
        .map(|l| l["requestId"].as_str().unwrap())
        .collect();
    assert_eq!(request_ids, ["req-a-3", "req-a-2", "req-a-1"]);
    assert!(
        lines
            .iter()
            .all(|l| l["id"].is_i64() && l["request"].is_object()),
        "whole samples"
    );

    assert!(audit_actions(&app)
        .await
        .contains(&"samples.export".to_string()));
    let entry = app
        .config
        .read(|conn| {
            audit::list(
                conn,
                &AuditQuery {
                    action: Some("samples.export".into()),
                    ..Default::default()
                },
                None,
                10,
            )
        })
        .await
        .unwrap()
        .value
        .remove(0)
        .entry;
    assert_eq!(entry.actor, "test-admin");
    assert!(
        entry.detail.contains("\"instance\":\"a\""),
        "{}",
        entry.detail
    );
}

#[tokio::test]
async fn the_export_is_not_taken_for_a_sample_id() {
    let app = TestApp::new().await;
    let (status, content_type, text) = app.get_text("/api/samples/export").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(content_type, "application/x-ndjson");
    assert!(text.is_empty(), "an empty table exports nothing");
}

#[tokio::test]
async fn a_large_export_is_complete_across_its_pages() {
    let app = TestApp::new().await;
    let batch: Vec<Sample> = (0..450).map(|n| sample(n, "a", "shop")).collect();
    app.store_samples(batch);
    let (_, _, text) = app.get_text("/api/samples/export").await;
    assert_eq!(text.lines().count(), 450, "more than two reader pages");
    let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(first["requestId"], "req-a-449");
}

#[tokio::test]
async fn clearing_empties_the_table_says_how_many_and_is_audited_and_announced() {
    let app = TestApp::new().await;
    stored(&app);
    let mut events = app.state.events.subscribe();
    let (status, body) = app.delete("/api/samples").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted"], 5);
    let (_, page) = app.get("/api/samples").await;
    assert!(ids(&page).is_empty());
    assert!(audit_actions(&app)
        .await
        .contains(&"samples.clear".to_string()));
    let Event::SamplingChanged { change, .. } = TestApp::next_event(&mut events).await else {
        panic!("expected a sampling event");
    };
    assert_eq!(change, "cleared");
    let (_, again) = app.delete("/api/samples").await;
    assert_eq!(again["deleted"], 0);
}

#[tokio::test]
async fn a_viewer_cannot_list_read_export_or_clear_samples() {
    let mut app = TestApp::new().await;
    stored(&app);
    let viewer = app.token("eyes", Role::Viewer).await;
    app.sign_in_with(viewer);
    for (method, uri) in [
        (Method::GET, "/api/samples"),
        (Method::GET, "/api/samples/1"),
        (Method::GET, "/api/samples/export"),
        (Method::DELETE, "/api/samples"),
    ] {
        let (status, _) = app.call(method.clone(), uri, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {uri}");
    }
    assert!(!audit_actions(&app)
        .await
        .contains(&"samples.clear".to_string()));
    // The settings and the status are readable, so the UI can say what is going on.
    assert_eq!(app.get("/api/sampling").await.0, StatusCode::OK);
}

#[tokio::test]
async fn without_a_token_nothing_is_readable() {
    let mut app = TestApp::new().await;
    stored(&app);
    app.sign_out();
    assert_eq!(app.get("/api/samples").await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(app.delete("/api/samples").await.0, StatusCode::UNAUTHORIZED);
}
