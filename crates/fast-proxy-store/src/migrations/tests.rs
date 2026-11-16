use std::path::Path;

use fast_proxy_model::{
    MaintenanceSettings, MaintenanceTarget, Role, SecretHash, TokenName, Trigger,
};
use rusqlite::types::Value;
use rusqlite::Connection;

use super::schema_shape::shape_of;
use super::*;
use crate::repository::audit::{self, AuditOutcome, NewAuditEntry};
use crate::repository::maintenance;
use crate::repository::samples::{self, NewSample};
use crate::repository::tokens::{self, NewToken};
use crate::test_support::seed;
use crate::Database;

/// The schema v0.1.1 created, exactly as released.
const SCHEMA_V5: &str = include_str!("fixtures/schema_v5.sql");

/// Every schema a release created, as released: each must upgrade.
const RELEASED: &[(i64, &str)] = &[(5, SCHEMA_V5), (6, include_str!("fixtures/schema_v6.sql"))];

/// A file as a release left it: its schema, in WAL mode.
fn old_file(path: &Path, version: i64) {
    let (_, schema) = RELEASED.iter().find(|(v, _)| *v == version).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
        .unwrap();
    conn.execute_batch(schema).unwrap();
    conn.pragma_update(None, "user_version", version).unwrap();
}

/// A file as v0.1.1 left it.
fn v5_file(path: &Path) {
    old_file(path, 5);
}

fn version_of(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

/// A current database with rows in every table the repositories reach.
fn populated(path: &Path) -> Database {
    let mut db = Database::open(path).unwrap();
    seed(&mut db, "shop", "shop.example.com");
    seed(&mut db, "blog", "*.blog.example.com");
    db.write_unrecorded(|conn| {
        tokens::create(
            conn,
            &NewToken {
                id: None,
                name: TokenName::parse("ops").unwrap(),
                role: Role::Admin,
                secret_hash: SecretHash::new([7; 32]),
                created_by: "cli".into(),
                expires_at_ms: Some(9_000_000),
            },
            1_000,
        )?;
        audit::append(
            conn,
            &NewAuditEntry {
                at_ms: 1_000,
                instance: "a".into(),
                actor: "ops".into(),
                role: "admin".into(),
                source: "127.0.0.1".into(),
                request_id: "r1".into(),
                action: "subsystem.create".into(),
                target: "shop".into(),
                outcome: AuditOutcome::Ok,
                detail: "{}".into(),
                revision: Some(1),
            },
        )?;
        maintenance::set(
            conn,
            MaintenanceTarget::Global,
            &MaintenanceSettings {
                message: "deploying".into(),
                body: None,
                retry_after_secs: Some(30),
                bypass: Default::default(),
                expires_at_ms: None,
                set_by: "ops".into(),
                set_at_ms: 1_000,
            },
        )?;
        samples::insert_batch(
            conn,
            &[NewSample {
                at_ms: 1_000,
                instance: "a".into(),
                request_id: "r2".into(),
                triggers: [Trigger::Slow].into_iter().collect(),
                subsystem: "shop".into(),
                route: "/".into(),
                client: "10.0.0.1".into(),
                method: "GET".into(),
                status: 200,
                total_ms: 2_500,
                detail: "{}".into(),
            }],
        )
    })
    .unwrap();
    db
}

fn table_names(conn: &Connection, schema: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT name FROM {schema}.sqlite_schema \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        ))
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn column_list(conn: &Connection, schema: &str, table: &str) -> String {
    let mut statement = conn
        .prepare("SELECT name FROM pragma_table_info(?1, ?2) ORDER BY cid")
        .unwrap();
    let columns: Vec<String> = statement
        .query_map([table, schema], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Writes a file of schema `version` at `old` holding, for every table that
/// schema has, the rows of `current`: what that release would have stored for
/// the same configuration.
fn copy_into(current: &Path, old: &Path, version: i64) {
    old_file(old, version);
    let conn = Connection::open(current).unwrap();
    conn.pragma_update(None, "foreign_keys", false).unwrap();
    conn.execute("ATTACH DATABASE ?1 AS old", [old.to_str().unwrap()])
        .unwrap();
    for table in table_names(&conn, "old") {
        let columns = column_list(&conn, "old", &table);
        conn.execute_batch(&format!(
            "INSERT OR REPLACE INTO old.\"{table}\" ({columns}) SELECT {columns} FROM main.\"{table}\""
        ))
        .unwrap();
    }
    conn.execute("DETACH DATABASE old", []).unwrap();
}

/// Every row of `table`, limited to the columns schema 5 has, in a stable order.
fn rows_of(path: &Path, table: &str, columns: &str) -> Vec<Vec<Value>> {
    let conn = Connection::open(path).unwrap();
    let mut statement = conn
        .prepare(&format!(
            "SELECT {columns} FROM \"{table}\" ORDER BY {columns}"
        ))
        .unwrap();
    let width = statement.column_count();
    statement
        .query_map([], |row| (0..width).map(|i| row.get(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn the_steps_lead_from_the_oldest_upgradable_version_to_the_current_one() {
    let targets: Vec<i64> = STEPS.iter().map(|step| step.to).collect();
    let expected: Vec<i64> = (OLDEST_UPGRADABLE + 1..=SCHEMA_VERSION).collect();
    assert_eq!(
        targets, expected,
        "one step per version, in order, none missing"
    );
}

#[test]
fn a_new_database_and_one_upgraded_from_every_release_have_the_same_shape() {
    let dir = tempfile::tempdir().unwrap();
    let new = dir.path().join("new.db");
    drop(Database::open(&new).unwrap());
    let expected = shape_of(&Connection::open(&new).unwrap());
    for (version, _) in RELEASED {
        let upgraded = dir.path().join(format!("v{version}.db"));
        old_file(&upgraded, *version);
        let opened = Database::open(&upgraded).unwrap();
        assert_eq!(
            opened.schema_upgrade().map(|u| (u.from, u.to)),
            Some((*version, SCHEMA_VERSION))
        );
        assert_eq!(
            shape_of(&Connection::open(&upgraded).unwrap()),
            expected,
            "upgraded from {version}"
        );
    }
}

#[test]
fn an_upgrade_from_every_release_keeps_every_row_and_the_whole_configuration() {
    for (version, _) in RELEASED {
        upgrade_keeps_every_row(*version);
    }
}

fn upgrade_keeps_every_row(version: i64) {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("current.db");
    let old = dir.path().join("old.db");
    let original = populated(&current).load_all().unwrap();
    copy_into(&current, &old, version);

    let upgraded = Database::open(&old).unwrap();
    assert_eq!(upgraded.schema_upgrade().map(|u| u.from), Some(version));
    assert_eq!(upgraded.load_all().unwrap(), original, "from {version}");

    let conn = Connection::open(&old).unwrap();
    let mut filled = Vec::new();
    for table in tables_of(version) {
        let columns = column_list(&conn, "main", &table);
        let before = rows_of(&current, &table, &columns);
        assert_eq!(
            rows_of(&old, &table, &columns),
            before,
            "table {table} from {version}"
        );
        if !before.is_empty() {
            filled.push(table);
        }
    }
    for table in [
        "subsystem",
        "subsystem_host",
        "upstream_group",
        "upstream_endpoint",
        "route",
        "revision",
        "admin_token",
        "audit_log",
        "maintenance",
        "sample",
    ] {
        assert!(
            filled.iter().any(|t| t == table),
            "{table} had rows to carry over"
        );
    }
}

/// The tables the schema of `version` has.
fn tables_of(version: i64) -> Vec<String> {
    let (_, schema) = RELEASED.iter().find(|(v, _)| *v == version).unwrap();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(schema).unwrap();
    table_names(&conn, "main")
}

/// Rewrites every stored revision as v0.1.1 wrote it: its JSON had none of the
/// fields later schemas added (`acme_enabled` in 6; the limits in 7).
fn revisions_as_v0_1_1_wrote_them(path: &Path) {
    let conn = Connection::open(path).unwrap();
    let mut paths = Vec::new();
    for i in 0..8 {
        let subsystem = format!("$.subsystems[{i}]");
        paths.push(format!("'{subsystem}.https.acme_enabled'"));
        paths.push(format!("'{subsystem}.rate_limit'"));
        for j in 0..8 {
            paths.push(format!("'{subsystem}.routes[{j}].rate_limit'"));
            paths.push(format!("'{subsystem}.routes[{j}].max_body_bytes'"));
            paths.push(format!(
                "'{subsystem}.upstream_groups[{j}].policy.max_requests'"
            ));
        }
    }
    let changed = conn
        .execute(
            &format!(
                "UPDATE revision SET config = json_remove(config, {})",
                paths.join(", ")
            ),
            [],
        )
        .unwrap();
    assert!(changed > 0);
    let left: i64 = conn
        .query_row(
            "SELECT count(*) FROM revision WHERE config LIKE '%acme_enabled%'
               OR config LIKE '%rate_limit%' OR config LIKE '%max_body_bytes%'
               OR config LIKE '%max_requests%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "no revision mentions the new field");
}

#[test]
fn an_upgraded_database_rolls_back_to_a_revision_the_old_release_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("current.db");
    let old = dir.path().join("old.db");
    drop(populated(&current));
    copy_into(&current, &old, 5);
    revisions_as_v0_1_1_wrote_them(&old);

    let mut upgraded = Database::open(&old).unwrap();
    let first = upgraded
        .read(|conn| crate::revision::get(conn, 1))
        .expect("a revision without the new field decodes");
    upgraded
        .write(
            |conn| {
                crate::revision::restore(conn, 1)?;
                Ok(crate::Mutation {
                    value: (),
                    summary: "rollback to v1".into(),
                })
            },
            |_| Ok(()),
        )
        .expect("and restores");
    assert_eq!(upgraded.load_all().unwrap(), first.config);
    assert!(first
        .config
        .subsystems
        .iter()
        .all(|s| !s.https.acme_enabled));
}

#[test]
fn an_upgraded_subsystem_has_acme_off() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("current.db");
    let old = dir.path().join("old.db");
    drop(populated(&current));
    copy_into(&current, &old, 5);
    let config = Database::open(&old).unwrap().load_all().unwrap();
    assert!(!config.subsystems.is_empty());
    assert!(config.subsystems.iter().all(|s| !s.https.acme_enabled));
}

#[test]
fn the_copy_taken_before_an_upgrade_is_the_old_database_readable_by_its_owner_only() {
    let dir = tempfile::tempdir().unwrap();
    let current = dir.path().join("current.db");
    let old = dir.path().join("fast-proxy.db");
    drop(populated(&current));
    copy_into(&current, &old, 5);

    let opened = Database::open(&old).unwrap();
    let copy = opened.schema_upgrade().unwrap().backup.clone().unwrap();
    assert_eq!(copy.parent(), old.parent());
    let name = copy.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with(&format!("fast-proxy.db.pre-v{SCHEMA_VERSION}-"))
            && name.ends_with(".bak"),
        "{name}"
    );
    assert_eq!(version_of(&copy), 5);
    let columns = column_list(&Connection::open(&copy).unwrap(), "main", "subsystem");
    assert_eq!(rows_of(&copy, "subsystem", &columns).len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&copy).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn an_in_memory_database_is_never_copied() {
    let mut conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_V5).unwrap();
    conn.pragma_update(None, "user_version", 5).unwrap();
    let upgrade = upgrade(&mut conn, None, 5).unwrap();
    assert_eq!(upgrade.backup, None);
}

#[test]
fn a_failed_upgrade_leaves_the_file_at_its_old_version() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.db");
    // Version 5 by its number, but without the tables a step alters.
    Connection::open(&broken)
        .unwrap()
        .pragma_update(None, "user_version", 5)
        .unwrap();
    assert!(Database::open(&broken).is_err());
    assert_eq!(version_of(&broken), 5);
    let conn = Connection::open(&broken).unwrap();
    assert!(
        table_names(&conn, "main").is_empty(),
        "nothing half-applied"
    );
}

#[test]
fn a_database_another_process_upgraded_meanwhile_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    v5_file(&path);
    // This connection read version 5, then another process upgraded the file.
    let mut late = Connection::open(&path).unwrap();
    drop(Database::open(&path).unwrap());
    upgrade(&mut late, None, 5).unwrap();
    assert_eq!(version_of(&path), SCHEMA_VERSION);
}

#[test]
fn two_processes_upgrading_one_file_at_once_both_open_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    v5_file(&path);
    let openers: Vec<_> = (0..4)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || Database::open(&path).map(|db| db.load_all().is_ok()))
        })
        .collect();
    for opener in openers {
        assert!(opener.join().unwrap().unwrap());
    }
    assert_eq!(version_of(&path), SCHEMA_VERSION);
}

#[test]
fn opening_a_current_database_reports_no_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.db");
    assert!(
        Database::open(&path).unwrap().schema_upgrade().is_none(),
        "a new file"
    );
    assert!(
        Database::open(&path).unwrap().schema_upgrade().is_none(),
        "reopened"
    );
    v5_file(&dir.path().join("old.db"));
    drop(Database::open(&dir.path().join("old.db")).unwrap());
    assert!(
        Database::open(&dir.path().join("old.db"))
            .unwrap()
            .schema_upgrade()
            .is_none(),
        "upgraded once, then current"
    );
}
