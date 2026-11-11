//! Compiling maintenance scopes into the snapshot.

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    BypassRules, Config, MaintenanceSettings, MaintenanceTarget, RouteId, SubsystemId,
};

use crate::compile::tests::{route, subsystem};
use crate::{
    compile_all, CompileError, Lookup, MaintenanceInputs, MaintenanceScope, Snapshot, TlsInputs,
};

fn on(message: &str, expires_at_ms: Option<i64>) -> MaintenanceSettings {
    MaintenanceSettings {
        message: message.into(),
        body: None,
        retry_after_secs: None,
        bypass: BypassRules::default(),
        expires_at_ms,
        set_by: "ops".into(),
        set_at_ms: 0,
    }
}

fn config() -> Config {
    let mut shop = subsystem(1, "shop", &["shop.example.com"]);
    shop.routes.push(route(101, "/api", 10));
    Config {
        subsystems: vec![shop, subsystem(2, "blog", &["blog.example.com"])],
    }
}

fn compile(
    scopes: Vec<(MaintenanceTarget, MaintenanceSettings)>,
    now_ms: i64,
) -> Result<Snapshot, CompileError> {
    compile_all(
        1,
        &config(),
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &MaintenanceInputs { scopes, now_ms },
    )
}

#[test]
fn with_no_scopes_nothing_is_in_maintenance() {
    let snapshot = compile(vec![], 0).unwrap();
    assert!(snapshot.maintenance.is_none());
    assert!(snapshot.subsystems.values().all(|s| s.maintenance.is_none()
        && s.routes
            .find("/")
            .is_none_or(|m| m.route.maintenance.is_none())));
}

#[test]
fn each_scope_lands_where_it_belongs() {
    let snapshot = compile(
        vec![
            (MaintenanceTarget::Global, on("g", None)),
            (MaintenanceTarget::Subsystem(SubsystemId(1)), on("s", None)),
            (MaintenanceTarget::Route(RouteId(101)), on("r", None)),
        ],
        0,
    )
    .unwrap();
    assert_eq!(
        snapshot.maintenance.as_ref().unwrap().scope,
        MaintenanceScope::Global
    );
    let shop = &snapshot.subsystems[&SubsystemId(1)];
    assert_eq!(
        shop.maintenance.as_ref().unwrap().scope,
        MaintenanceScope::Subsystem
    );
    assert!(snapshot.subsystems[&SubsystemId(2)].maintenance.is_none());

    let Lookup::Matched { route, .. } = snapshot.lookup("shop.example.com", "/api/x") else {
        panic!("matched")
    };
    assert_eq!(
        route.route.maintenance.as_ref().unwrap().scope,
        MaintenanceScope::Route
    );
    let Lookup::Matched { route, .. } = snapshot.lookup("shop.example.com", "/other") else {
        panic!("matched")
    };
    assert!(route.route.maintenance.is_none(), "only the /api route");
}

#[test]
fn a_scope_already_over_is_left_out_and_one_that_ends_later_is_kept() {
    let snapshot = compile(
        vec![
            (MaintenanceTarget::Global, on("over", Some(1_000))),
            (
                MaintenanceTarget::Subsystem(SubsystemId(1)),
                on("later", Some(5_000)),
            ),
        ],
        1_000,
    )
    .unwrap();
    assert!(
        snapshot.maintenance.is_none(),
        "expired at the compile instant"
    );
    let kept = snapshot.subsystems[&SubsystemId(1)]
        .maintenance
        .as_ref()
        .unwrap();
    assert_eq!(kept.expires_at_ms(), Some(5_000));
}

#[test]
fn a_scope_for_a_missing_subsystem_or_route_is_rejected_naming_it() {
    let err = compile(
        vec![(MaintenanceTarget::Subsystem(SubsystemId(99)), on("x", None))],
        0,
    )
    .unwrap_err();
    assert_eq!(err, CompileError::UnknownMaintenanceSubsystem { id: 99 });
    let err = compile(
        vec![(MaintenanceTarget::Route(RouteId(999)), on("x", None))],
        0,
    )
    .unwrap_err();
    assert_eq!(err, CompileError::UnknownMaintenanceRoute { id: 999 });
    assert!(err.to_string().contains("999"));
}

#[test]
fn a_dangling_scope_is_rejected_even_when_it_has_already_expired() {
    let err = compile(
        vec![(
            MaintenanceTarget::Subsystem(SubsystemId(99)),
            on("x", Some(1)),
        )],
        1_000_000,
    )
    .unwrap_err();
    assert_eq!(err, CompileError::UnknownMaintenanceSubsystem { id: 99 });
}

#[test]
fn a_disabled_route_may_hold_a_scope_that_is_simply_unused() {
    let mut config = config();
    config.subsystems[0].routes[1].enabled = false;
    let snapshot = compile_all(
        1,
        &config,
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &MaintenanceInputs {
            scopes: vec![(MaintenanceTarget::Route(RouteId(101)), on("r", None))],
            now_ms: 0,
        },
    )
    .unwrap();
    assert!(snapshot.maintenance.is_none());
}

#[test]
fn compiling_maintenance_keeps_endpoint_runtime_state() {
    let first = compile(vec![], 0).unwrap();
    let registry = first.runtime_registry();
    let second = compile_all(
        2,
        &config(),
        &registry,
        &TlsInputs::off(),
        &MaintenanceInputs {
            scopes: vec![(MaintenanceTarget::Global, on("g", None))],
            now_ms: 0,
        },
    )
    .unwrap();
    let runtime = |snapshot: &Snapshot| {
        snapshot.subsystems[&SubsystemId(1)]
            .upstreams
            .values()
            .next()
            .unwrap()
            .candidates[0]
            .runtime
            .clone()
    };
    assert!(std::sync::Arc::ptr_eq(&runtime(&first), &runtime(&second)));
}
