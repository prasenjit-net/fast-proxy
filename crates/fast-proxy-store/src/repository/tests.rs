use fast_proxy_model::{HostId, RouteId, SubsystemId, UpstreamGroupId};

use super::{hosts, routes, subsystems, upstreams};
use crate::test_support::{commit, host, route_fields, seed, subsystem_fields, upstream_fields};
use crate::{Database, StoreError, SubsystemFields};

fn db() -> Database {
    Database::open_in_memory().unwrap()
}

fn count(db: &Database, table: &str) -> i64 {
    db.read(|conn| {
        Ok(
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })?,
        )
    })
    .unwrap()
}

// ---- subsystems -------------------------------------------------------

#[test]
fn creates_gets_and_lists_subsystems() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    seed(&mut db, "beta", "b.example.com");

    let got = db.read(|conn| subsystems::get(conn, a.subsystem)).unwrap();
    assert_eq!(got.name.as_str(), "alpha");
    assert_eq!(got.hosts.len(), 1);
    assert_eq!(got.upstream_groups.len(), 1);
    assert_eq!(got.routes.len(), 1);

    let listed = db.read(subsystems::list).unwrap();
    let names: Vec<&str> = listed.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"]);
}

#[test]
fn a_duplicate_subsystem_name_is_a_conflict() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    let err = commit(&mut db, "dup", |conn| {
        subsystems::create(conn, &subsystem_fields("alpha"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err}");
}

#[test]
fn updates_a_subsystem_and_reports_missing_ones() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    commit(&mut db, "edit", |conn| {
        subsystems::update(
            conn,
            a.subsystem,
            &SubsystemFields {
                enabled: false,
                description: "off".into(),
                ..subsystem_fields("gamma")
            },
        )
    })
    .unwrap();
    let got = db.read(|conn| subsystems::get(conn, a.subsystem)).unwrap();
    assert_eq!(got.name.as_str(), "gamma");
    assert!(!got.enabled);
    assert_eq!(got.description, "off");

    let err = commit(&mut db, "edit", |conn| {
        subsystems::update(conn, SubsystemId(999), &subsystem_fields("x"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn renaming_onto_an_existing_name_is_a_conflict() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    seed(&mut db, "beta", "b.example.com");
    let err = commit(&mut db, "rename", |conn| {
        subsystems::update(conn, a.subsystem, &subsystem_fields("beta"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
}

#[test]
fn getting_or_deleting_a_missing_subsystem_is_not_found() {
    let mut db = db();
    let err = db
        .read(|conn| subsystems::get(conn, SubsystemId(1)))
        .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
    let err = commit(&mut db, "del", |conn| {
        subsystems::delete(conn, SubsystemId(1))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
}

#[test]
fn deleting_a_subsystem_cascades_to_everything_under_it() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    seed(&mut db, "beta", "b.example.com");
    commit(&mut db, "del", |conn| subsystems::delete(conn, a.subsystem)).unwrap();

    for (table, expected) in [
        ("subsystem", 1),
        ("subsystem_host", 1),
        ("upstream_group", 1),
        ("upstream_endpoint", 1),
        ("route", 1),
    ] {
        assert_eq!(count(&db, table), expected, "{table}");
    }
}

// ---- hosts ------------------------------------------------------------

#[test]
fn adds_lists_and_removes_hosts() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let added = commit(&mut db, "host", |conn| {
        hosts::add(conn, a.subsystem, &host("*.example.org"))
    })
    .unwrap()
    .value;

    let listed = db.read(|conn| hosts::list(conn, a.subsystem)).unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1].0, added);
    assert_eq!(listed[1].1.to_pattern_string(), "*.example.org");

    commit(&mut db, "rm", |conn| {
        hosts::remove(conn, a.subsystem, added)
    })
    .unwrap();
    assert_eq!(db.read(|c| hosts::list(c, a.subsystem)).unwrap().len(), 1);
}

#[test]
fn a_host_pattern_is_unique_across_all_subsystems() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    let err = commit(&mut db, "dup", |conn| {
        hosts::add(conn, b.subsystem, &host("a.example.com"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
}

#[test]
fn host_operations_on_missing_things_are_not_found() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    let host_of_a = db.read(|c| hosts::list(c, a.subsystem)).unwrap()[0].0;

    let add = commit(&mut db, "x", |c| {
        hosts::add(c, SubsystemId(999), &host("x.com"))
    });
    assert!(matches!(add.unwrap_err(), StoreError::NotFound(_)));
    let list = db.read(|c| hosts::list(c, SubsystemId(999)));
    assert!(matches!(list.unwrap_err(), StoreError::NotFound(_)));
    let missing = commit(&mut db, "x", |c| hosts::remove(c, a.subsystem, HostId(999)));
    assert!(matches!(missing.unwrap_err(), StoreError::NotFound(_)));
    // A host belonging to another subsystem cannot be removed through this one.
    let wrong_owner = commit(&mut db, "x", |c| hosts::remove(c, b.subsystem, host_of_a));
    assert!(matches!(wrong_owner.unwrap_err(), StoreError::NotFound(_)));
}

// ---- upstreams --------------------------------------------------------

#[test]
fn creates_and_updates_an_upstream_with_its_single_endpoint() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let id = commit(&mut db, "up", |conn| {
        upstreams::create(
            conn,
            a.subsystem,
            &upstream_fields("api", "api.internal:8080"),
        )
    })
    .unwrap()
    .value;
    commit(&mut db, "up", |conn| {
        upstreams::update(
            conn,
            a.subsystem,
            id,
            &upstream_fields("api2", "api2.internal:9090"),
        )
    })
    .unwrap();

    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    let group = got.upstream_groups.iter().find(|g| g.id == id).unwrap();
    assert_eq!(group.name, "api2");
    assert_eq!(group.endpoints.len(), 1);
    assert_eq!(group.endpoints[0].addr.to_string(), "api2.internal:9090");
    assert_eq!(count(&db, "upstream_endpoint"), 2);
}

#[test]
fn an_upstream_name_is_unique_within_a_subsystem_only() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    // "backend" exists in both seeds already; a second one in alpha conflicts.
    let err = commit(&mut db, "dup", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("backend", "x:1"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    // Renaming within beta to a fresh name works.
    commit(&mut db, "ok", |c| {
        upstreams::update(c, b.subsystem, b.group, &upstream_fields("other", "x:1"))
    })
    .unwrap();
}

#[test]
fn deleting_an_upstream_used_by_a_route_is_a_conflict() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let err = commit(&mut db, "del", |c| {
        upstreams::delete(c, a.subsystem, a.group)
    })
    .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("used by 1 route")),
        "{err}"
    );
    assert_eq!(count(&db, "upstream_group"), 1);
}

#[test]
fn deletes_an_unused_upstream_and_reports_missing_ones() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let spare = commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("spare", "x:1"))
    })
    .unwrap()
    .value;
    commit(&mut db, "del", |c| upstreams::delete(c, a.subsystem, spare)).unwrap();
    assert_eq!(count(&db, "upstream_group"), 1);
    assert_eq!(count(&db, "upstream_endpoint"), 1);

    let again = commit(&mut db, "del", |c| upstreams::delete(c, a.subsystem, spare));
    assert!(matches!(again.unwrap_err(), StoreError::NotFound(_)));
    let update = commit(&mut db, "up", |c| {
        upstreams::update(
            c,
            a.subsystem,
            UpstreamGroupId(999),
            &upstream_fields("x", "x:1"),
        )
    });
    assert!(matches!(update.unwrap_err(), StoreError::NotFound(_)));
    let create = commit(&mut db, "up", |c| {
        upstreams::create(c, SubsystemId(999), &upstream_fields("x", "x:1"))
    });
    assert!(matches!(create.unwrap_err(), StoreError::NotFound(_)));
}

// ---- routes -----------------------------------------------------------

#[test]
fn creates_updates_and_deletes_routes() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let id = commit(&mut db, "route", |c| {
        let mut fields = route_fields("/api", a.group);
        fields.strip_prefix = true;
        routes::create(c, a.subsystem, &fields)
    })
    .unwrap()
    .value;

    commit(&mut db, "route", |c| {
        let mut fields = route_fields("/api/v2", a.group);
        fields.enabled = false;
        fields.preserve_host = false;
        routes::update(c, a.subsystem, id, &fields)
    })
    .unwrap();
    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    let route = got.routes.iter().find(|r| r.id == id).unwrap();
    assert_eq!(route.prefix.as_str(), "/api/v2");
    assert!(!route.enabled);
    assert!(!route.preserve_host);
    assert!(!route.strip_prefix);

    commit(&mut db, "route", |c| routes::delete(c, a.subsystem, id)).unwrap();
    assert_eq!(count(&db, "route"), 1);
}

#[test]
fn a_route_prefix_is_unique_within_a_subsystem_only() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    let err = commit(&mut db, "dup", |c| {
        routes::create(c, a.subsystem, &route_fields("/", a.group))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    commit(&mut db, "ok", |c| {
        routes::create(c, b.subsystem, &route_fields("/other", b.group))
    })
    .unwrap();
}

#[test]
fn a_route_must_use_an_upstream_of_its_own_subsystem() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");

    let create = commit(&mut db, "x", |c| {
        routes::create(c, a.subsystem, &route_fields("/x", b.group))
    })
    .unwrap_err();
    assert!(matches!(&create, StoreError::Invalid(f) if f[0].field == "upstream_group_id"));

    let missing = commit(&mut db, "x", |c| {
        routes::create(c, a.subsystem, &route_fields("/x", UpstreamGroupId(999)))
    })
    .unwrap_err();
    assert!(matches!(missing, StoreError::Invalid(_)));

    let route = db.read(|c| subsystems::get(c, a.subsystem)).unwrap().routes[0].id;
    let update = commit(&mut db, "x", |c| {
        routes::update(c, a.subsystem, route, &route_fields("/", b.group))
    })
    .unwrap_err();
    assert!(matches!(update, StoreError::Invalid(_)));
}

#[test]
fn route_operations_on_missing_things_are_not_found() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    let route_of_a = db.read(|c| subsystems::get(c, a.subsystem)).unwrap().routes[0].id;

    let create = commit(&mut db, "x", |c| {
        routes::create(c, SubsystemId(999), &route_fields("/x", a.group))
    });
    assert!(matches!(create.unwrap_err(), StoreError::NotFound(_)));
    let update = commit(&mut db, "x", |c| {
        routes::update(c, a.subsystem, RouteId(999), &route_fields("/x", a.group))
    });
    assert!(matches!(update.unwrap_err(), StoreError::NotFound(_)));
    let delete = commit(&mut db, "x", |c| routes::delete(c, b.subsystem, route_of_a));
    assert!(matches!(delete.unwrap_err(), StoreError::NotFound(_)));
}

// ---- load_all ---------------------------------------------------------

#[test]
fn load_all_is_empty_for_a_fresh_database_and_ordered_by_id() {
    let mut db = db();
    assert!(db.load_all().unwrap().subsystems.is_empty());
    seed(&mut db, "zeta", "z.example.com");
    seed(&mut db, "alpha", "a.example.com");
    let names: Vec<String> = db
        .load_all()
        .unwrap()
        .subsystems
        .iter()
        .map(|s| s.name.as_str().to_string())
        .collect();
    assert_eq!(names, ["zeta", "alpha"]);
}

#[test]
fn corrupt_stored_values_are_reported_as_internal_errors() {
    let mut db = db();
    seed(&mut db, "alpha", "a.example.com");
    db.read(|c| {
        c.execute("UPDATE route SET prefix = 'no-slash'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.load_all().unwrap_err(),
        StoreError::Internal(_)
    ));
}
