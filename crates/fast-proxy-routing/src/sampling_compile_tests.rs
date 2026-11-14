//! Compiling the sampling settings into the snapshot.

use std::sync::Arc;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    Config, RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId,
};

use crate::compile::tests::{route, subsystem};
use crate::{
    compile_full, CompileError, MaintenanceInputs, SamplingInputs, SamplingRuntime, Snapshot,
    TlsInputs,
};

fn config() -> Config {
    let mut shop = subsystem(1, "shop", &["shop.example.com"]);
    shop.routes.push(route(101, "/api", 10));
    shop.routes.push(route(102, "/static", 10));
    Config {
        subsystems: vec![shop, subsystem(2, "blog", &["blog.example.com"])],
    }
}

fn an_override() -> SamplingOverride {
    SamplingOverride {
        enabled: None,
        capture_bodies: None,
        baseline_per_mille: None,
        slow_threshold_ms: None,
        slow_p99_factor_x10: None,
        set_by: "ops".into(),
        set_at_ms: 0,
    }
}

fn inputs(overrides: Vec<(SamplingTarget, SamplingOverride)>) -> SamplingInputs {
    SamplingInputs {
        overrides,
        ..SamplingInputs::none()
    }
}

fn compile(sampling: &SamplingInputs) -> Result<Snapshot, CompileError> {
    compile_full(
        1,
        &config(),
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &MaintenanceInputs::none(),
        sampling,
    )
}

fn route_policy(
    snapshot: &Snapshot,
    subsystem: u8,
    prefix: &str,
) -> Arc<fast_proxy_sampling::SamplingPolicy> {
    let id = SubsystemId(i64::from(subsystem));
    snapshot.subsystems[&id]
        .routes
        .find(prefix)
        .unwrap()
        .route
        .sampling
        .policy
        .clone()
}

#[test]
fn with_no_overrides_every_route_gets_the_global_policy() {
    let snapshot = compile(&SamplingInputs::none()).unwrap();
    let defaults = SamplingSettings::default();
    assert_eq!(*snapshot.sampling_settings, defaults);
    for (subsystem, prefix) in [(1, "/api"), (1, "/static")] {
        let policy = route_policy(&snapshot, subsystem, prefix);
        assert!(policy.enabled);
        assert!(
            !policy.capture_bodies,
            "bodies are off unless somebody turns them on"
        );
        assert_eq!(policy.baseline_per_mille, defaults.baseline_per_mille);
    }
    assert!(snapshot.unmatched_sampling.enabled);
    assert!(snapshot.subsystems.values().all(|s| s.sampling.enabled));
}

#[test]
fn overrides_land_on_their_subsystem_and_route_only() {
    let mut subsystem_override = an_override();
    subsystem_override.baseline_per_mille = Some(10);
    subsystem_override.capture_bodies = Some(true);
    let mut route_override = an_override();
    route_override.baseline_per_mille = Some(500);
    route_override.enabled = Some(false);
    let snapshot = compile(&inputs(vec![
        (
            SamplingTarget::Subsystem(SubsystemId(1)),
            subsystem_override,
        ),
        (SamplingTarget::Route(RouteId(102)), route_override),
    ]))
    .unwrap();

    let api = route_policy(&snapshot, 1, "/api");
    assert_eq!(api.baseline_per_mille, 10, "from the subsystem");
    assert!(api.capture_bodies);
    let stat = route_policy(&snapshot, 1, "/static");
    assert_eq!(stat.baseline_per_mille, 500, "the route's own wins");
    assert!(!stat.enabled);
    assert!(stat.capture_bodies, "and the rest comes from the subsystem");

    let shop = &snapshot.subsystems[&SubsystemId(1)];
    assert_eq!(
        shop.sampling.baseline_per_mille, 10,
        "requests no route took"
    );
    assert!(shop.sampling.capture_bodies);
    let blog = &snapshot.subsystems[&SubsystemId(2)];
    assert_eq!(
        blog.sampling.baseline_per_mille, 1,
        "another subsystem is untouched"
    );
    assert!(!blog.sampling.capture_bodies);
    assert_eq!(snapshot.unmatched_sampling.baseline_per_mille, 1);
}

#[test]
fn a_route_is_on_only_when_its_policy_is() {
    let mut off = an_override();
    off.enabled = Some(false);
    let snapshot = compile(&inputs(vec![(SamplingTarget::Route(RouteId(101)), off)])).unwrap();
    let shop = &snapshot.subsystems[&SubsystemId(1)];
    assert!(!shop.routes.find("/api").unwrap().route.sampling.is_on());
    assert!(shop.routes.find("/static").unwrap().route.sampling.is_on());
}

#[test]
fn globally_disabled_sampling_is_off_everywhere_that_does_not_override_it() {
    let settings = SamplingSettings {
        enabled: false,
        ..SamplingSettings::default()
    };
    let mut on = an_override();
    on.enabled = Some(true);
    let snapshot = compile(&SamplingInputs {
        settings,
        overrides: vec![(SamplingTarget::Route(RouteId(101)), on)],
        runtime: SamplingRuntime::new(),
    })
    .unwrap();
    assert!(route_policy(&snapshot, 1, "/api").enabled);
    assert!(!route_policy(&snapshot, 1, "/static").enabled);
    assert!(!snapshot.unmatched_sampling.enabled);
}

#[test]
fn an_override_for_something_that_does_not_exist_is_refused() {
    let err = compile(&inputs(vec![(
        SamplingTarget::Subsystem(SubsystemId(99)),
        an_override(),
    )]))
    .unwrap_err();
    assert_eq!(err, CompileError::UnknownSamplingSubsystem { id: 99 });
    let err = compile(&inputs(vec![(
        SamplingTarget::Route(RouteId(99)),
        an_override(),
    )]))
    .unwrap_err();
    assert_eq!(err, CompileError::UnknownSamplingRoute { id: 99 });
}

#[test]
fn the_runtime_state_is_carried_to_the_next_snapshot() {
    let first = compile(&SamplingInputs::none()).unwrap();
    let window = first.sampling.window("shop", "/api").unwrap();
    window.record(250, 1_000);
    let next = compile(&SamplingInputs {
        runtime: first.sampling_runtime(),
        ..SamplingInputs::none()
    })
    .unwrap();
    let carried = next.subsystems[&SubsystemId(1)]
        .routes
        .find("/api")
        .unwrap()
        .route
        .sampling
        .latency
        .clone();
    assert!(Arc::ptr_eq(&window, &carried), "the route keeps its window");
    assert!(Arc::ptr_eq(&first.sampling.clients, &next.sampling.clients));
    assert!(Arc::ptr_eq(
        &first.sampling.storm_guard,
        &next.sampling.storm_guard
    ));
    assert!(Arc::ptr_eq(
        &first.sampling.rate_limiter,
        &next.sampling.rate_limiter
    ));
}

#[test]
fn a_disabled_or_removed_route_has_no_window_and_a_new_one_starts_fresh() {
    let first = compile(&SamplingInputs::none()).unwrap();
    let api = first.sampling.window("shop", "/api").unwrap();
    let mut changed = config();
    changed.subsystems[0]
        .routes
        .retain(|r| r.prefix.as_str() != "/api");
    for route in &mut changed.subsystems[0].routes {
        if route.prefix.as_str() == "/static" {
            route.enabled = false;
        }
    }
    changed.subsystems[0].routes.push(route(103, "/api", 10));
    let next = compile_full(
        2,
        &changed,
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &MaintenanceInputs::none(),
        &SamplingInputs {
            runtime: first.sampling_runtime(),
            ..SamplingInputs::none()
        },
    )
    .unwrap();
    assert!(
        next.sampling.window("shop", "/static").is_none(),
        "a disabled route is not compiled"
    );
    let recreated = next.sampling.window("shop", "/api").unwrap();
    assert!(
        Arc::ptr_eq(&api, &recreated),
        "the key is the subsystem and the prefix, so a route with the same prefix keeps the history"
    );
}

#[test]
fn the_empty_snapshot_samples_nothing() {
    let empty = Snapshot::empty();
    assert!(!empty.unmatched_sampling.enabled);
    assert_eq!(empty.sampling.windows().count(), 0);
}
