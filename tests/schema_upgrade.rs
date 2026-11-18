//! The real binary upgrades a database an earlier release wrote: it keeps routing
//! the stored configuration, says so once, and leaves a copy of the old file.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use common::{console, create_site, http, start_backend, start_proxy};
use fast_proxy_store::Connection;

/// Turns a current database back into what v0.1.1 (schema 5) stored for the same
/// configuration: schema 6 added the ACME tables and one subsystem column, and
/// schema 7 the rate-limit policies and two caps.
fn back_to_schema_5(database: &Path) {
    let conn = Connection::open(database).unwrap();
    conn.execute_batch(
        "DROP TABLE rate_limit_policy;
         ALTER TABLE route DROP COLUMN max_body_bytes;
         ALTER TABLE upstream_group DROP COLUMN max_requests;
         DROP TABLE acme_account;
         DROP TABLE acme_challenge;
         DROP TABLE acme_lease;
         DROP TABLE acme_status;
         ALTER TABLE subsystem DROP COLUMN https_acme_enabled;
         PRAGMA user_version = 5;",
    )
    .unwrap();
}

fn wait_for_line(running: &console::Running, wanted: &str) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(line) = console::printed(running)
            .into_iter()
            .find(|line| line.contains(wanted))
        {
            return Some(line);
        }
    }
    None
}

#[test]
fn a_database_from_the_previous_release_is_upgraded_and_keeps_serving() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_backend("old-site");
    {
        let proxy = start_proxy(dir.path());
        create_site(&proxy, "shop", "shop.example.com", backend);
    }
    let database = dir.path().join("state").join("fast-proxy.db");
    back_to_schema_5(&database);

    let running = console::start_ok(dir.path(), &[]);
    let line = wait_for_line(&running, "upgraded the database schema from version 5 to ")
        .expect("the upgrade is reported on the console");
    assert!(line.contains("fast-proxy.db.pre-v"), "{line}");

    let (status, body) = http(running.data, "GET", "/", "shop.example.com", None);
    assert_eq!((status, body.as_str()), (200, "old-site /"));

    let copies: Vec<_> = std::fs::read_dir(dir.path().join("state"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("fast-proxy.db.pre-v"))
        .collect();
    assert_eq!(copies.len(), 1, "{copies:?}");
    let version: i64 = Connection::open(dir.path().join("state").join(&copies[0]))
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 5, "the copy is the database as it was");
}

#[test]
fn a_database_from_a_newer_release_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    drop(start_proxy(dir.path()));
    let database = dir.path().join("state").join("fast-proxy.db");
    Connection::open(&database)
        .unwrap()
        .pragma_update(None, "user_version", 99)
        .unwrap();

    assert!(
        console::start(dir.path(), &[], &[]).is_err(),
        "it does not start"
    );
    let version: i64 = Connection::open(&database)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 99);
}
