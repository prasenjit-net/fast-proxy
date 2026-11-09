use fast_proxy_model::Config;

use super::{get, list};
use crate::repository::{routes, subsystems};
use crate::test_support::{commit, route_fields, seed, subsystem_fields};
use crate::{Database, Mutation, StoreError};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn restore_as_new_revision(db: &mut Database, version: u64) -> u64 {
    db.write(
        |conn| {
            super::restore(conn, version)?;
            Ok(Mutation {
                value: (),
                summary: format!("rollback to v{version}"),
            })
        },
        |_| Ok(()),
    )
    .unwrap()
    .version
}

#[test]
fn a_fresh_database_has_no_revisions() {
    let db = db();
    assert_eq!(db.current_version().unwrap(), 0);
    assert!(db.read(|c| list(c, 10, None)).unwrap().is_empty());
}

#[test]
fn every_write_appends_a_revision_with_its_summary_and_config() {
    let mut db = db();
    let written = commit(&mut db, "subsystem created: shop", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap();
    assert_eq!(written.version, 1);
    assert_eq!(written.config.subsystems.len(), 1);

    commit(&mut db, "subsystem deleted: shop", |c| {
        subsystems::delete(c, written.value)
    })
    .unwrap();
    assert_eq!(db.current_version().unwrap(), 2);

    let first = db.read(|c| get(c, 1)).unwrap();
    assert_eq!(first.summary, "subsystem created: shop");
    assert_eq!(first.config.subsystems[0].name.as_str(), "shop");
    let second = db.read(|c| get(c, 2)).unwrap();
    assert!(second.config.subsystems.is_empty());
}

#[test]
fn failed_writes_leave_no_revision() {
    let mut db = db();
    seed(&mut db, "shop", "s.example.com");
    let before = db.current_version().unwrap();
    commit(&mut db, "dup", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap_err();
    assert_eq!(db.current_version().unwrap(), before);
}

#[test]
fn lists_newest_first_and_pages_with_before() {
    let mut db = db();
    for name in ["a", "b", "c", "d"] {
        commit(&mut db, &format!("create {name}"), |c| {
            subsystems::create(c, &subsystem_fields(name))
        })
        .unwrap();
    }
    let page = db.read(|c| list(c, 2, None)).unwrap();
    let versions: Vec<u64> = page.iter().map(|r| r.version).collect();
    assert_eq!(versions, [4, 3]);
    assert_eq!(page[0].summary, "create d");

    let next = db.read(|c| list(c, 2, Some(3))).unwrap();
    let versions: Vec<u64> = next.iter().map(|r| r.version).collect();
    assert_eq!(versions, [2, 1]);
}

#[test]
fn a_missing_revision_is_not_found() {
    let db = db();
    let err = db.read(|c| get(c, 5)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
    let err = db.read(|c| super::restore(c, 5)).unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn only_the_last_100_revisions_are_kept() {
    let mut db = db();
    let id = commit(&mut db, "create", |c| {
        subsystems::create(c, &subsystem_fields("shop"))
    })
    .unwrap()
    .value;
    for n in 0..109 {
        commit(&mut db, &format!("edit {n}"), |c| {
            let mut fields = subsystem_fields("shop");
            fields.description = n.to_string();
            subsystems::update(c, id, &fields)
        })
        .unwrap();
    }
    assert_eq!(db.current_version().unwrap(), 110);
    let kept = db.read(|c| list(c, 500, None)).unwrap();
    assert_eq!(kept.len(), 100);
    assert_eq!(kept.last().unwrap().version, 11);
    assert!(db.read(|c| get(c, 10)).is_err());
}

#[test]
fn rollback_restores_each_prior_version_exactly() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com"); // v1
    let route = commit(&mut db, "route", |c| {
        routes::create(c, a.subsystem, &route_fields("/api", a.group))
    })
    .unwrap(); // v2
    commit(&mut db, "edit", |c| {
        let mut fields = subsystem_fields("alpha");
        fields.enabled = false;
        subsystems::update(c, a.subsystem, &fields)
    })
    .unwrap(); // v3
    commit(&mut db, "route removed", |c| {
        routes::delete(c, a.subsystem, route.value)
    })
    .unwrap(); // v4
    seed(&mut db, "beta", "b.example.com"); // v5
    commit(&mut db, "delete alpha", |c| {
        subsystems::delete(c, a.subsystem)
    })
    .unwrap(); // v6

    let stored: Vec<Config> = (1..=6)
        .map(|v| db.read(|c| get(c, v)).unwrap().config)
        .collect();
    for version in (1..=6u64).rev() {
        let new_version = restore_as_new_revision(&mut db, version);
        assert_eq!(
            db.load_all().unwrap(),
            stored[(version - 1) as usize],
            "after rolling back to v{version}"
        );
        assert_eq!(db.current_version().unwrap(), new_version);
        let latest = db.read(|c| get(c, new_version)).unwrap();
        assert_eq!(latest.summary, format!("rollback to v{version}"));
        assert_eq!(latest.config, stored[(version - 1) as usize]);
    }
}

#[test]
fn rollback_keeps_ids_so_new_rows_never_reuse_them() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    commit(&mut db, "delete", |c| subsystems::delete(c, a.subsystem)).unwrap();
    restore_as_new_revision(&mut db, 1);
    let restored = db.load_all().unwrap();
    assert_eq!(restored.subsystems[0].id, a.subsystem);
    assert_eq!(restored.subsystems[0].routes.len(), 1);
}

#[test]
fn an_unreadable_stored_revision_is_an_internal_error() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    db.read(|c| {
        c.execute("UPDATE revision SET config = 'not json'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.read(|c| get(c, 1)).unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn an_invalid_value_inside_a_stored_revision_is_an_internal_error() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    db.read(|c| {
        c.execute(
            "UPDATE revision SET config = replace(config, '\"alpha\"', '\"Bad Name\"')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.read(|c| get(c, 1)).unwrap_err(),
        StoreError::Internal(_)
    ));
}
