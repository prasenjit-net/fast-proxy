//! Maintenance mode against the real binary: turned on over the admin API it
//! answers 503 with `Retry-After` and never reaches the backend, a TTL ends it
//! by itself (and the sweeper clears the stored row), and it survives a restart.

mod common;

use common::*;
use serde_json::json;

#[test]
fn maintenance_answers_503_ends_by_itself_and_is_cleared_from_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 200);

    let (status, body) = admin(
        &proxy,
        "PUT",
        "/api/maintenance/global",
        Some(json!({ "message": "deploying", "ttlSecs": 2 })),
    );
    assert_eq!(status, 200, "{body}");
    let (status, text) = through_proxy(&proxy, "shop.example.com", "/");
    assert_eq!(status, 503);
    assert!(text.contains("deploying"), "{text}");

    // Waiting on the event (requests served again), not on a fixed sleep.
    assert!(
        wait_until(|| through_proxy(&proxy, "shop.example.com", "/").0 == 200),
        "the TTL ends maintenance with nobody looking"
    );
    // The sweeper then clears the stored row, so the API agrees.
    assert!(wait_until(|| {
        let (_, list) = admin(&proxy, "GET", "/api/maintenance", None);
        list["data"] == json!([])
    }));
}

#[test]
fn maintenance_survives_a_restart_and_turning_it_off_restores_service() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_backend("shop-backend");
    {
        let proxy = start_proxy(dir.path());
        create_site(&proxy, "shop", "shop.example.com", backend);
        let (status, _) = admin(
            &proxy,
            "PUT",
            "/api/subsystems/1/maintenance",
            Some(json!({ "message": "persisted" })),
        );
        assert_eq!(status, 200);
        assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 503);
    }
    let proxy = start_proxy(dir.path());
    assert_eq!(
        through_proxy(&proxy, "shop.example.com", "/").0,
        503,
        "a restart during maintenance stays in maintenance"
    );
    let (status, _) = admin(&proxy, "DELETE", "/api/subsystems/1/maintenance", None);
    assert_eq!(status, 200);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 200);
}

#[test]
fn the_metrics_and_the_status_show_maintenance() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    let backend = start_backend("shop-backend");
    create_site(&proxy, "shop", "shop.example.com", backend);
    admin(&proxy, "PUT", "/api/maintenance/global", Some(json!({})));
    for _ in 0..3 {
        assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 503);
    }
    assert!(wait_until(|| {
        metric(&proxy, "fastproxy_maintenance_responses_total").is_some_and(|n| n >= 3.0)
    }));
    assert_eq!(
        metric(
            &proxy,
            r#"fastproxy_maintenance_active{route="_none",scope="global""#
        ),
        Some(1.0)
    );
    let (_, status) = admin(&proxy, "GET", "/api/status", None);
    assert_eq!(status["data"]["admin"]["activeMaintenance"], 1);
}
