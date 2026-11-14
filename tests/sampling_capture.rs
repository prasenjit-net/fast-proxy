//! Request sampling against the real binary: an unusual request ends up in the
//! shared database, redacted, even when the proxy is stopped right after it, and
//! sampling can be switched off.

mod common;

use std::path::Path;

use common::*;
use fast_proxy_store::repository::samples::{self, SampleFilter, SampleSummary};
use fast_proxy_store::Database;

const HOST: &str = "shop.example.com";

fn database(dir: &Path) -> std::path::PathBuf {
    dir.join("state").join("fast-proxy.db")
}

/// Every sample in the database, newest first, read the way another process
/// (an admin API on another instance) would.
fn stored(dir: &Path) -> Vec<SampleSummary> {
    let Ok(db) = Database::open_read_only(&database(dir)) else {
        return Vec::new();
    };
    db.read(|conn| {
        samples::list(
            conn,
            &SampleFilter {
                limit: Some(100),
                ..Default::default()
            },
        )
    })
    .unwrap_or_default()
}

/// A site whose only backend is a port nothing listens on, so every request is
/// a connectivity failure.
fn site_with_a_dead_backend(proxy: &Proxy) {
    create_site(proxy, "shop", HOST, free_port());
}

#[test]
fn a_connectivity_failure_is_stored_in_the_database_and_counted() {
    let dir = tempfile::tempdir().unwrap();
    let proxy = start_proxy(dir.path());
    site_with_a_dead_backend(&proxy);

    let (status, _) = through_proxy(&proxy, HOST, "/orders?page=2");
    assert_eq!(status, 502);

    assert!(
        wait_until(|| !stored(dir.path()).is_empty()),
        "the batch is written within its interval"
    );
    let sample = stored(dir.path()).remove(0);
    assert_eq!(sample.primary.as_str(), "upstream_failure");
    assert_eq!(
        (sample.subsystem.as_str(), sample.route.as_str()),
        ("shop", "/")
    );
    assert_eq!((sample.method.as_str(), sample.status), ("GET", 502));
    assert!(!sample.request_id.is_empty());
    assert!(
        wait_until(|| metric(
            &proxy,
            "fastproxy_samples_captured_total{trigger=\"upstream_failure\"}"
        ) == Some(1.0)),
        "the capture is counted"
    );
    assert!(wait_until(|| metric(
        &proxy,
        "fastproxy_samples_written_total"
    ) == Some(1.0)));
}

#[cfg(unix)]
#[test]
fn a_sample_still_waiting_for_its_batch_is_written_when_the_proxy_stops() {
    let dir = tempfile::tempdir().unwrap();
    // A batch interval far longer than the test: only shutdown can write it.
    let mut proxy = start_on_free_ports(dir.path(), "", "\n[sampling]\nbatch_interval = \"60s\"\n");
    site_with_a_dead_backend(&proxy);
    assert_eq!(through_proxy(&proxy, HOST, "/").0, 502);
    assert!(
        wait_until(|| metric(
            &proxy,
            "fastproxy_samples_captured_total{trigger=\"upstream_failure\"}"
        ) == Some(1.0)),
        "kept, and queued for its batch"
    );
    assert!(stored(dir.path()).is_empty(), "not written yet");

    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, std::time::Duration::from_secs(20)),
        Some(true)
    );
    assert_eq!(stored(dir.path()).len(), 1, "flushed at shutdown");
}

#[cfg(unix)]
#[test]
fn sampling_can_be_turned_off_and_then_nothing_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = try_start_with_args(
        dir.path(),
        free_port(),
        free_port(),
        ("", ""),
        &["--no-sampling"],
    )
    .unwrap();
    site_with_a_dead_backend(&proxy);
    assert_eq!(through_proxy(&proxy, HOST, "/").0, 502);
    assert_eq!(through_proxy(&proxy, HOST, "/").0, 502);
    assert_eq!(metric(&proxy, "fastproxy_samples_captured_total"), None);
    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, std::time::Duration::from_secs(20)),
        Some(true)
    );
    assert!(stored(dir.path()).is_empty());
}

#[cfg(unix)]
#[test]
fn no_secret_is_in_the_database_file_or_its_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut proxy = start_proxy(dir.path());
    site_with_a_dead_backend(&proxy);
    let (status, _) = http_with_headers(
        proxy.data,
        "GET",
        "/login?token=ZQTOKEN-in-the-query&user=ann",
        HOST,
        &[
            ("Authorization", "Bearer ZQBEARER"),
            ("Cookie", "session=ZQCOOKIE"),
            ("X-Api-Key", "ZQAPIKEY"),
        ],
        None,
    );
    assert_eq!(status, 502);
    assert!(wait_until(|| !stored(dir.path()).is_empty()));
    interrupt(&proxy);
    assert_eq!(
        wait_for_exit(&mut proxy, std::time::Duration::from_secs(20)),
        Some(true)
    );

    let mut scanned = 0;
    for entry in std::fs::read_dir(dir.path().join("state"))
        .unwrap()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("fast-proxy.db") {
            continue;
        }
        scanned += 1;
        let bytes = std::fs::read(entry.path()).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("ZQ"), "{name} holds a secret");
    }
    assert!(scanned >= 1, "the database file was read");
}
