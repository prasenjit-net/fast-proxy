use fast_proxy_model::{
    EndpointId, EndpointState, FailurePolicy, HostId, LbAlgorithm, RouteId, SubsystemId,
    UpstreamGroupId,
};

use super::{endpoints, hosts, routes, subsystems, upstreams};
use crate::test_support::{
    commit, create_group, endpoint_fields, host, route_fields, seed, subsystem_fields,
    upstream_fields,
};
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

#[test]
fn timestamps_are_recorded_and_advance_when_children_change() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let first = db.read(subsystems::times).unwrap()[&a.subsystem];
    assert!(first.created_at_ms > 0);
    assert!(first.updated_at_ms >= first.created_at_ms);

    std::thread::sleep(std::time::Duration::from_millis(5));
    commit(&mut db, "host", |c| {
        hosts::add(c, a.subsystem, &host("b.example.com"))
    })
    .unwrap();
    let second = db.read(subsystems::times).unwrap()[&a.subsystem];
    assert_eq!(second.created_at_ms, first.created_at_ms);
    assert!(second.updated_at_ms > first.updated_at_ms);
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
fn creates_a_group_with_its_endpoints_and_policy_and_updates_the_settings() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let mut fields = upstream_fields("api");
    fields.policy.lb.algorithm = LbAlgorithm::LeastConnections;
    let id = commit(&mut db, "up", |conn| {
        upstreams::create(
            conn,
            a.subsystem,
            &fields,
            &[
                endpoint_fields("api1.internal:8080"),
                endpoint_fields("api2.internal:8080"),
            ],
        )
    })
    .unwrap()
    .value;

    let mut renamed = upstream_fields("api2");
    renamed.policy.failure = FailurePolicy::FailClosed;
    commit(&mut db, "up", |conn| {
        upstreams::update(conn, a.subsystem, id, &renamed)
    })
    .unwrap();

    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    let group = got.upstream_groups.iter().find(|g| g.id == id).unwrap();
    assert_eq!(group.name, "api2");
    assert_eq!(group.policy.failure, FailurePolicy::FailClosed);
    assert_eq!(
        group.policy.lb.algorithm,
        LbAlgorithm::RoundRobin,
        "update replaces the policy"
    );
    assert_eq!(group.endpoints.len(), 2, "update leaves endpoints alone");
    assert_eq!(count(&db, "upstream_endpoint"), 3);
}

#[test]
fn a_group_needs_one_to_64_endpoints_and_unique_addresses() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let none = commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("none"), &[])
    })
    .unwrap_err();
    assert!(matches!(&none, StoreError::Invalid(f) if f[0].field == "endpoints"));

    let many: Vec<_> = (0..65)
        .map(|i| endpoint_fields(&format!("h{i}:1")))
        .collect();
    let too_many = commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("many"), &many)
    })
    .unwrap_err();
    assert!(matches!(&too_many, StoreError::Invalid(f) if f[0].field == "endpoints"));
    commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("max"), &many[..64])
    })
    .unwrap();

    let dup = commit(&mut db, "up", |c| {
        upstreams::create(
            c,
            a.subsystem,
            &upstream_fields("dup"),
            &[endpoint_fields("x:1"), endpoint_fields("x:1")],
        )
    })
    .unwrap_err();
    assert!(matches!(dup, StoreError::Conflict(_)));
    assert_eq!(
        count(&db, "upstream_group"),
        2,
        "the failed create left nothing behind"
    );
}

#[test]
fn an_invalid_policy_or_weight_is_rejected_with_its_field() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let mut bad_policy = upstream_fields("p");
    bad_policy.policy.passive.max_ejection_percent = 0;
    let err = commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &bad_policy, &[endpoint_fields("x:1")])
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "passive.maxEjectionPercent"));

    let mut heavy = endpoint_fields("x:1");
    heavy.weight = 1001;
    let err = commit(&mut db, "up", |c| {
        upstreams::create(c, a.subsystem, &upstream_fields("w"), &[heavy])
    })
    .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "endpoints.0.weight"));
}

#[test]
fn an_upstream_name_is_unique_within_a_subsystem_only() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    // "backend" exists in both seeds already; a second one in alpha conflicts.
    let err = commit(&mut db, "dup", |c| {
        create_group(c, a.subsystem, "backend", "x:1")
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    // Renaming within beta to a fresh name works.
    commit(&mut db, "ok", |c| {
        upstreams::update(c, b.subsystem, b.group, &upstream_fields("other"))
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
        create_group(c, a.subsystem, "spare", "x:1")
    })
    .unwrap()
    .value;
    commit(&mut db, "del", |c| upstreams::delete(c, a.subsystem, spare)).unwrap();
    assert_eq!(count(&db, "upstream_group"), 1);
    assert_eq!(count(&db, "upstream_endpoint"), 1);

    let again = commit(&mut db, "del", |c| upstreams::delete(c, a.subsystem, spare));
    assert!(matches!(again.unwrap_err(), StoreError::NotFound(_)));
    let update = commit(&mut db, "up", |c| {
        upstreams::update(c, a.subsystem, UpstreamGroupId(999), &upstream_fields("x"))
    });
    assert!(matches!(update.unwrap_err(), StoreError::NotFound(_)));
    let create = commit(&mut db, "up", |c| {
        create_group(c, SubsystemId(999), "x", "x:1")
    });
    assert!(matches!(create.unwrap_err(), StoreError::NotFound(_)));
}

// ---- endpoints --------------------------------------------------------

fn only_endpoint(db: &Database, a: &crate::test_support::Fixture) -> EndpointId {
    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    got.upstream_groups[0].endpoints[0].id
}

#[test]
fn adds_updates_sets_state_and_removes_endpoints() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let first = only_endpoint(&db, &a);
    let second = commit(&mut db, "add", |c| {
        endpoints::add(c, a.subsystem, a.group, &endpoint_fields("b:2"))
    })
    .unwrap()
    .value;
    let mut changed = endpoint_fields("c:3");
    changed.weight = 5;
    commit(&mut db, "edit", |c| {
        endpoints::update(c, a.subsystem, a.group, second, &changed)
    })
    .unwrap();
    commit(&mut db, "drain", |c| {
        endpoints::set_state(c, a.subsystem, a.group, second, EndpointState::Draining)
    })
    .unwrap();

    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    let endpoint = got.upstream_groups[0]
        .endpoints
        .iter()
        .find(|e| e.id == second)
        .unwrap();
    assert_eq!(endpoint.addr.to_string(), "c:3");
    assert_eq!(endpoint.weight, 5);
    assert_eq!(endpoint.state, EndpointState::Draining);

    commit(&mut db, "rm", |c| {
        endpoints::delete(c, a.subsystem, a.group, second)
    })
    .unwrap();
    let got = db.read(|c| subsystems::get(c, a.subsystem)).unwrap();
    assert_eq!(got.upstream_groups[0].endpoints[0].id, first);
    assert_eq!(got.upstream_groups[0].endpoints.len(), 1);
}

#[test]
fn the_last_endpoint_cannot_be_removed() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let only = only_endpoint(&db, &a);
    let err = commit(&mut db, "rm", |c| {
        endpoints::delete(c, a.subsystem, a.group, only)
    })
    .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("last endpoint")),
        "{err}"
    );
    assert_eq!(count(&db, "upstream_endpoint"), 1);
}

#[test]
fn endpoint_writes_reject_duplicates_missing_rows_and_other_groups() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    let b = seed(&mut db, "beta", "b.example.com");
    let dup = commit(&mut db, "add", |c| {
        endpoints::add(c, a.subsystem, a.group, &endpoint_fields("127.0.0.1:3000"))
    })
    .unwrap_err();
    assert!(matches!(dup, StoreError::Conflict(_)));

    let foreign = only_endpoint(&db, &b);
    let wrong_group = commit(&mut db, "state", |c| {
        endpoints::set_state(c, a.subsystem, a.group, foreign, EndpointState::Disabled)
    });
    assert!(matches!(wrong_group.unwrap_err(), StoreError::NotFound(_)));
    let wrong_subsystem = commit(&mut db, "add", |c| {
        endpoints::add(c, b.subsystem, a.group, &endpoint_fields("z:9"))
    });
    assert!(matches!(
        wrong_subsystem.unwrap_err(),
        StoreError::NotFound(_)
    ));
    let missing = commit(&mut db, "edit", |c| {
        endpoints::update(
            c,
            a.subsystem,
            a.group,
            EndpointId(999),
            &endpoint_fields("z:9"),
        )
    });
    assert!(matches!(missing.unwrap_err(), StoreError::NotFound(_)));
}

#[test]
fn a_group_holds_at_most_64_endpoints() {
    let mut db = db();
    let a = seed(&mut db, "alpha", "a.example.com");
    for i in 1..64 {
        commit(&mut db, "add", |c| {
            endpoints::add(
                c,
                a.subsystem,
                a.group,
                &endpoint_fields(&format!("h{i}:1")),
            )
        })
        .unwrap();
    }
    let err = commit(&mut db, "add", |c| {
        endpoints::add(c, a.subsystem, a.group, &endpoint_fields("one-more:1"))
    })
    .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
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
