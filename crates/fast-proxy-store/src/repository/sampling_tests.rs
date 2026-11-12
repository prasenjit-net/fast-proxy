use fast_proxy_model::{
    RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId, Trigger, TriggerSet,
};

use super::sampling;
use super::subsystems;
use crate::test_support::{commit, seed};
use crate::{Database, StoreError};

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

fn an_override(baseline: u32) -> SamplingOverride {
    SamplingOverride {
        enabled: Some(true),
        capture_bodies: Some(true),
        baseline_per_mille: Some(baseline),
        slow_threshold_ms: Some(500),
        slow_p99_factor_x10: None,
        set_by: "ops".into(),
        set_at_ms: 7,
    }
}

fn changed_settings() -> SamplingSettings {
    SamplingSettings {
        enabled: false,
        triggers: [Trigger::SlowP99, Trigger::Baseline].into_iter().collect(),
        slow_threshold_ms: 750,
        slow_p99_factor_x10: 45,
        slow_p99_floor_ms: 80,
        client_errors_per_min: 250,
        oversized_request_bytes: 1 << 20,
        oversized_response_bytes: 5 << 30,
        baseline_per_mille: 25,
        rate_per_sec: 100,
        rate_burst: 200,
        storm_burst: 5,
        storm_interval_secs: 60,
        retention_days: 30,
        max_rows: 5_000_000,
        max_bytes: 8 << 30,
        body_request_cap: 8192,
        body_response_cap: 16_384,
        extra_headers: vec!["x-tenant".into()],
        redact_headers: vec!["x-internal-key".into()],
        redact_query_params: vec!["sessionid".into()],
        redact_body_fields: vec!["iban".into(), "pin".into()],
        updated_by: "ann".into(),
        updated_at_ms: 99,
    }
}

#[test]
fn before_anything_is_written_the_defaults_are_read() {
    let (db, _, _) = setup();
    assert_eq!(
        db.read(sampling::get_settings).unwrap(),
        SamplingSettings::default()
    );
}

#[test]
fn every_global_setting_round_trips() {
    let (db, _, _) = setup();
    db.read(|c| sampling::set_settings(c, &changed_settings()))
        .unwrap();
    assert_eq!(db.read(sampling::get_settings).unwrap(), changed_settings());
}

#[test]
fn writing_the_settings_again_replaces_them() {
    let (db, _, _) = setup();
    db.read(|c| sampling::set_settings(c, &changed_settings()))
        .unwrap();
    db.read(|c| sampling::set_settings(c, &SamplingSettings::default()))
        .unwrap();
    assert_eq!(
        db.read(sampling::get_settings).unwrap(),
        SamplingSettings::default()
    );
    let rows: i64 = db
        .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM sampling_settings", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn invalid_settings_are_refused_with_the_field_and_nothing_is_stored() {
    let (db, _, _) = setup();
    let mut bad = changed_settings();
    bad.baseline_per_mille = 5000;
    let err = db.read(|c| sampling::set_settings(c, &bad)).unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "baselinePerMille"));
    assert_eq!(
        db.read(sampling::get_settings).unwrap(),
        SamplingSettings::default()
    );
}

#[test]
fn stored_settings_that_no_longer_validate_are_corruption() {
    let (db, _, _) = setup();
    db.read(|c| sampling::set_settings(c, &changed_settings()))
        .unwrap();
    db.read(|c| {
        c.execute("UPDATE sampling_settings SET triggers = 4096", [])?;
        Ok(())
    })
    .unwrap();
    let err = db.read(sampling::get_settings).unwrap_err();
    assert!(matches!(err, StoreError::Internal(m) if m.contains("trigger")));
    db.read(|c| {
        c.execute(
            "UPDATE sampling_settings SET triggers = 1, rate_per_sec = 0",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        db.read(sampling::get_settings).unwrap_err(),
        StoreError::Internal(_)
    ));
}

#[test]
fn an_override_can_be_set_for_a_subsystem_and_a_route_and_listed() {
    let (db, subsystem, route) = setup();
    let targets = [
        SamplingTarget::Subsystem(subsystem),
        SamplingTarget::Route(route),
    ];
    for target in targets {
        db.read(|c| sampling::set_override(c, target, &an_override(10)))
            .unwrap();
    }
    let rows = db.read(sampling::list_overrides).unwrap();
    assert_eq!(rows.len(), 2);
    for (row, target) in rows.iter().zip(targets) {
        assert_eq!(row.target, target);
        assert_eq!(row.settings, an_override(10));
    }
}

#[test]
fn an_override_keeps_inherited_fields_empty() {
    let (db, subsystem, _) = setup();
    let mut sparse = an_override(0);
    sparse.enabled = None;
    sparse.capture_bodies = None;
    sparse.slow_threshold_ms = None;
    let target = SamplingTarget::Subsystem(subsystem);
    db.read(|c| sampling::set_override(c, target, &sparse))
        .unwrap();
    assert_eq!(
        db.read(sampling::list_overrides).unwrap()[0].settings,
        sparse
    );
}

#[test]
fn setting_an_override_again_replaces_it() {
    let (db, subsystem, _) = setup();
    let target = SamplingTarget::Subsystem(subsystem);
    db.read(|c| sampling::set_override(c, target, &an_override(1)))
        .unwrap();
    db.read(|c| sampling::set_override(c, target, &an_override(2)))
        .unwrap();
    let rows = db.read(sampling::list_overrides).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings.baseline_per_mille, Some(2));
}

#[test]
fn clearing_reports_whether_there_was_an_override() {
    let (db, subsystem, route) = setup();
    let target = SamplingTarget::Subsystem(subsystem);
    db.read(|c| sampling::set_override(c, target, &an_override(1)))
        .unwrap();
    db.read(|c| sampling::set_override(c, SamplingTarget::Route(route), &an_override(2)))
        .unwrap();
    assert!(db.read(|c| sampling::clear_override(c, target)).unwrap());
    assert!(!db.read(|c| sampling::clear_override(c, target)).unwrap());
    let rows = db.read(sampling::list_overrides).unwrap();
    assert_eq!(rows.len(), 1, "the other override stays");
    assert_eq!(rows[0].target, SamplingTarget::Route(route));
}

#[test]
fn a_missing_target_is_not_found_and_invalid_input_names_the_field() {
    let (db, subsystem, _) = setup();
    for target in [
        SamplingTarget::Subsystem(SubsystemId(99)),
        SamplingTarget::Route(RouteId(99)),
    ] {
        let err = db
            .read(|c| sampling::set_override(c, target, &an_override(1)))
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)), "{target:?}");
    }
    let mut bad = an_override(1);
    bad.slow_threshold_ms = Some(0);
    let err = db
        .read(|c| sampling::set_override(c, SamplingTarget::Subsystem(subsystem), &bad))
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "slowThresholdMs"));
    assert!(db.read(sampling::list_overrides).unwrap().is_empty());
}

#[test]
fn deleting_a_subsystem_removes_its_overrides_and_its_routes() {
    let (mut db, subsystem, route) = setup();
    db.read(|c| sampling::set_override(c, SamplingTarget::Subsystem(subsystem), &an_override(1)))
        .unwrap();
    db.read(|c| sampling::set_override(c, SamplingTarget::Route(route), &an_override(2)))
        .unwrap();
    commit(&mut db, "delete", |c| subsystems::delete(c, subsystem)).unwrap();
    assert!(db.read(sampling::list_overrides).unwrap().is_empty());
}

#[test]
fn deleting_a_route_removes_only_its_override() {
    let (mut db, subsystem, route) = setup();
    db.read(|c| sampling::set_override(c, SamplingTarget::Subsystem(subsystem), &an_override(1)))
        .unwrap();
    db.read(|c| sampling::set_override(c, SamplingTarget::Route(route), &an_override(2)))
        .unwrap();
    commit(&mut db, "delete route", |c| {
        c.execute("DELETE FROM route WHERE id = ?1", [route.0])?;
        Ok(())
    })
    .unwrap();
    let rows = db.read(sampling::list_overrides).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, SamplingTarget::Subsystem(subsystem));
}

#[test]
fn the_default_trigger_set_is_what_a_fresh_reader_sees() {
    let (db, _, _) = setup();
    let settings = db.read(sampling::get_settings).unwrap();
    assert_eq!(settings.triggers, TriggerSet::defaults());
}
