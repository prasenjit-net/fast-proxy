use fast_proxy_model::{
    BodyContentType, BypassHeader, BypassRules, Cidr, CustomBody, MaintenanceSettings,
    MaintenanceTarget, RouteId, SecretHash, SubsystemId,
};

use super::maintenance;
use super::{routes, subsystems};
use crate::test_support::{commit, seed, subsystem_fields};
use crate::{Database, StoreError};

fn settings(message: &str) -> MaintenanceSettings {
    MaintenanceSettings {
        message: message.into(),
        body: None,
        retry_after_secs: None,
        bypass: BypassRules::default(),
        expires_at_ms: None,
        set_by: "ops".into(),
        set_at_ms: 100,
    }
}

fn full_settings() -> MaintenanceSettings {
    MaintenanceSettings {
        message: "deploying".into(),
        body: Some(CustomBody {
            content_type: BodyContentType::Json,
            text: "{\"down\":true}".into(),
        }),
        retry_after_secs: Some(30),
        bypass: BypassRules {
            cidrs: vec![
                Cidr::parse("bypass.cidrs", "10.0.0.0/8").unwrap(),
                Cidr::parse("bypass.cidrs", "2001:db8::/32").unwrap(),
            ],
            header: Some(BypassHeader {
                name: "X-Pass".into(),
                secret_hash: SecretHash::new([5; 32]),
            }),
        },
        expires_at_ms: Some(9_999),
        set_by: "ops".into(),
        set_at_ms: 100,
    }
}

fn setup() -> (Database, SubsystemId, RouteId) {
    let mut db = Database::open_in_memory().unwrap();
    let fixture = seed(&mut db, "shop", "shop.example.com");
    let route = db
        .load_all()
        .unwrap()
        .subsystems
        .remove(0)
        .routes
        .remove(0)
        .id;
    (db, fixture.subsystem, route)
}

#[test]
fn every_scope_can_be_set_and_listed_with_all_its_settings() {
    let (db, subsystem, route) = setup();
    let targets = [
        MaintenanceTarget::Global,
        MaintenanceTarget::Subsystem(subsystem),
        MaintenanceTarget::Route(route),
    ];
    for target in targets {
        db.read(|c| maintenance::set(c, target, &full_settings()))
            .unwrap();
    }
    let rows = db.read(maintenance::list).unwrap();
    assert_eq!(rows.len(), 3);
    for (row, target) in rows.iter().zip(targets) {
        assert_eq!(row.target, target);
        assert_eq!(row.settings, full_settings());
    }
}

#[test]
fn setting_a_scope_again_replaces_it() {
    let (db, subsystem, _) = setup();
    let target = MaintenanceTarget::Subsystem(subsystem);
    db.read(|c| maintenance::set(c, target, &settings("first")))
        .unwrap();
    db.read(|c| maintenance::set(c, target, &settings("second")))
        .unwrap();
    let rows = db.read(maintenance::list).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings.message, "second");
}

#[test]
fn clearing_reports_whether_anything_was_active() {
    let (db, _, _) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("x")))
        .unwrap();
    assert!(db
        .read(|c| maintenance::clear(c, MaintenanceTarget::Global))
        .unwrap());
    assert!(!db
        .read(|c| maintenance::clear(c, MaintenanceTarget::Global))
        .unwrap());
    assert!(db.read(maintenance::list).unwrap().is_empty());
}

#[test]
fn clearing_one_scope_leaves_the_others() {
    let (db, subsystem, _) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("g")))
        .unwrap();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Subsystem(subsystem), &settings("s")))
        .unwrap();
    db.read(|c| maintenance::clear(c, MaintenanceTarget::Global))
        .unwrap();
    let rows = db.read(maintenance::list).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings.message, "s");
}

#[test]
fn a_missing_target_is_not_found() {
    let (db, _, _) = setup();
    for target in [
        MaintenanceTarget::Subsystem(SubsystemId(99)),
        MaintenanceTarget::Route(RouteId(99)),
    ] {
        let err = db
            .read(|c| maintenance::set(c, target, &settings("x")))
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)), "{target:?}");
    }
}

#[test]
fn invalid_settings_are_refused_with_the_field() {
    let (db, _, _) = setup();
    let mut bad = settings("x");
    bad.retry_after_secs = Some(0);
    let err = db
        .read(|c| maintenance::set(c, MaintenanceTarget::Global, &bad))
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "retryAfterSecs"));
    assert!(db.read(maintenance::list).unwrap().is_empty());
}

#[test]
fn deleting_a_subsystem_or_route_removes_its_scope() {
    let (mut db, subsystem, route) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Route(route), &settings("r")))
        .unwrap();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("g")))
        .unwrap();
    commit(&mut db, "delete route", |c| {
        routes::delete(c, subsystem, route)
    })
    .unwrap();
    let rows = db.read(maintenance::list).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, MaintenanceTarget::Global);

    db.read(|c| maintenance::set(c, MaintenanceTarget::Subsystem(subsystem), &settings("s")))
        .unwrap();
    commit(&mut db, "delete subsystem", |c| {
        subsystems::delete(c, subsystem)
    })
    .unwrap();
    let rows = db.read(maintenance::list).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, MaintenanceTarget::Global);
}

#[test]
fn a_row_that_no_longer_validates_is_internal() {
    let (db, _, _) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("x")))
        .unwrap();
    let corrupt = |sql: &str| {
        db.read(|c| {
            c.execute(sql, []).map_err(StoreError::from)?;
            maintenance::list(c)
        })
        .unwrap_err()
    };
    let err = corrupt("UPDATE maintenance SET scope = 'route'");
    assert!(matches!(err, StoreError::Internal(m) if m.contains("do not match")));
    let err = corrupt("UPDATE maintenance SET scope = 'global', retry_after_secs = 0");
    assert!(matches!(err, StoreError::Internal(_)));
    let err = corrupt("UPDATE maintenance SET retry_after_secs = NULL, bypass_cidrs = 'nope'");
    assert!(matches!(err, StoreError::Internal(_)));
    let err = corrupt("UPDATE maintenance SET bypass_cidrs = '[]', bypass_header = 'X-A'");
    assert!(matches!(err, StoreError::Internal(m) if m.contains("secret hash")));
}

#[test]
fn only_one_row_per_target_can_exist() {
    let (db, _, _) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("x")))
        .unwrap();
    let duplicate = db.read(|c| {
        c.execute(
            "INSERT INTO maintenance (scope, set_by, set_at) VALUES ('global', 'x', 1)",
            [],
        )
        .map_err(StoreError::from)
    });
    assert!(matches!(duplicate.unwrap_err(), StoreError::Conflict(_)));
}

#[test]
fn a_new_subsystem_is_unaffected_by_other_scopes() {
    let (mut db, _, _) = setup();
    db.read(|c| maintenance::set(c, MaintenanceTarget::Global, &settings("g")))
        .unwrap();
    let written = commit(&mut db, "create other", |c| {
        subsystems::create(c, &subsystem_fields("other"))
    })
    .unwrap();
    assert_eq!(db.read(maintenance::list).unwrap().len(), 1);
    assert!(written.version > 0);
}
