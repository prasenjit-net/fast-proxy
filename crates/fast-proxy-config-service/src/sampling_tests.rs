//! Sampling settings through the config service: revisions, audit, the veto,
//! the cluster, a restart, a rollback, and the runtime state across changes.

use std::path::Path;
use std::sync::Arc;

use fast_proxy_model::{
    RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId, Trigger, TriggerSet,
};
use fast_proxy_store::repository::audit::{self, AuditEntry, AuditQuery};
use fast_proxy_store::Store;
use fast_proxy_telemetry::SyncResult;
use tempfile::TempDir;

use crate::tests::create_site;
use crate::{Audited, ChangeContext, ConfigService};

fn ann() -> ChangeContext {
    ChangeContext::token("ann", "admin", "10.0.0.5", "req-1")
}

async fn start_at(path: &Path) -> Arc<ConfigService> {
    Arc::new(
        ConfigService::start(Store::open(path).unwrap())
            .await
            .unwrap(),
    )
}

struct Cluster {
    _dir: TempDir,
    path: std::path::PathBuf,
    a: Arc<ConfigService>,
    b: Arc<ConfigService>,
}

impl Cluster {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let a = start_at(&path).await;
        let b = start_at(&path).await;
        Self {
            _dir: dir,
            path,
            a,
            b,
        }
    }
}

async fn memory() -> Arc<ConfigService> {
    Arc::new(
        ConfigService::start(Store::open_in_memory().unwrap())
            .await
            .unwrap(),
    )
}

async fn add_site(service: &ConfigService, name: &'static str) -> SubsystemId {
    service
        .apply(
            &ann(),
            Audited::new("subsystem.create", name),
            create_site(name, format!("{name}.example.com"), "127.0.0.1:3000"),
        )
        .await
        .unwrap()
        .value
}

async fn audit_rows(service: &ConfigService) -> Vec<AuditEntry> {
    service
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 100))
        .await
        .unwrap()
        .value
}

fn baseline_override(per_mille: u32) -> SamplingOverride {
    SamplingOverride {
        enabled: None,
        capture_bodies: Some(true),
        baseline_per_mille: Some(per_mille),
        slow_threshold_ms: None,
        slow_p99_factor_x10: None,
        set_by: "somebody else".into(),
        set_at_ms: 1,
    }
}

fn settings(retention_days: u32) -> SamplingSettings {
    SamplingSettings {
        retention_days,
        triggers: [Trigger::Slow, Trigger::Baseline].into_iter().collect(),
        updated_by: "somebody else".into(),
        ..SamplingSettings::default()
    }
}

/// The policy of the `/` route of a subsystem in the live snapshot.
fn route_policy(
    service: &ConfigService,
    subsystem: SubsystemId,
) -> Arc<fast_proxy_sampling::SamplingPolicy> {
    service.snapshot_holder().load().subsystems[&subsystem]
        .routes
        .find("/")
        .unwrap()
        .route
        .sampling
        .policy
        .clone()
}

fn route_id(service: &ConfigService, subsystem: SubsystemId) -> RouteId {
    service.snapshot_holder().load().subsystems[&subsystem]
        .routes
        .find("/")
        .unwrap()
        .route
        .id
}

#[tokio::test]
async fn before_anything_is_written_the_defaults_apply() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    let (stored, overrides) = service.sampling().await.unwrap();
    assert_eq!(stored, SamplingSettings::default());
    assert!(overrides.is_empty());
    assert!(route_policy(&service, shop).enabled);
}

#[tokio::test]
async fn changing_the_settings_is_a_revision_with_an_audit_entry_and_goes_live() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    let before = service.snapshot_holder().load().version;

    let applied = service.set_sampling(&ann(), settings(3)).await.unwrap();

    assert_eq!(applied.version, before + 1);
    let live = service.snapshot_holder().load();
    assert_eq!(live.version, applied.version);
    assert_eq!(live.sampling_settings.retention_days, 3);
    assert_eq!(
        route_policy(&service, shop).triggers,
        [Trigger::Slow, Trigger::Baseline]
            .into_iter()
            .collect::<TriggerSet>()
    );
    let entries = audit_rows(&service).await;
    let entry = &entries
        .iter()
        .find(|e| e.entry.action == "sampling.update")
        .unwrap()
        .entry;
    assert_eq!(entry.actor, "ann");
    assert_eq!(entry.target, "global");
    assert_eq!(entry.revision, Some(applied.version));
}

#[tokio::test]
async fn who_changed_the_settings_and_when_come_from_the_context() {
    let service = memory().await;
    service.set_sampling(&ann(), settings(3)).await.unwrap();
    let (stored, _) = service.sampling().await.unwrap();
    assert_eq!(
        stored.updated_by, "ann",
        "not what the caller wrote in the struct"
    );
    assert!(stored.updated_at_ms > 0);
}

#[tokio::test]
async fn an_override_can_be_set_replaced_and_cleared() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    let route = route_id(&service, shop);

    service
        .set_sampling_override(&ann(), SamplingTarget::Route(route), baseline_override(500))
        .await
        .unwrap();
    let policy = route_policy(&service, shop);
    assert_eq!(policy.baseline_per_mille, 500);
    assert!(policy.capture_bodies);
    let (_, overrides) = service.sampling().await.unwrap();
    assert_eq!(overrides[0].settings.set_by, "ann");

    service
        .set_sampling_override(&ann(), SamplingTarget::Route(route), baseline_override(7))
        .await
        .unwrap();
    assert_eq!(route_policy(&service, shop).baseline_per_mille, 7);

    let cleared = service
        .clear_sampling_override(&ann(), SamplingTarget::Route(route))
        .await
        .unwrap();
    assert!(cleared.value);
    assert!(!route_policy(&service, shop).capture_bodies);

    let actions: Vec<String> = audit_rows(&service)
        .await
        .into_iter()
        .map(|e| e.entry.action)
        .collect();
    for expected in ["sampling.override.set", "sampling.override.clear"] {
        assert!(
            actions.iter().any(|a| a == expected),
            "{expected} in {actions:?}"
        );
    }
    let target = audit_rows(&service)
        .await
        .into_iter()
        .find(|e| e.entry.action == "sampling.override.set")
        .unwrap()
        .entry
        .target;
    assert_eq!(target, format!("route:{}", route.0));
}

#[tokio::test]
async fn clearing_an_override_that_is_not_there_writes_nothing() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    let version = service.snapshot_holder().load().version;
    let audit_before = audit_rows(&service).await.len();
    let cleared = service
        .clear_sampling_override(&ann(), SamplingTarget::Subsystem(shop))
        .await
        .unwrap();
    assert!(!cleared.value);
    assert_eq!(cleared.version, version);
    assert_eq!(service.snapshot_holder().load().version, version);
    assert_eq!(audit_rows(&service).await.len(), audit_before);
}

#[tokio::test]
async fn a_vetoed_change_leaves_the_settings_the_snapshot_and_the_log_as_they_were() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    service.set_sampling(&ann(), settings(3)).await.unwrap();
    let version = service.snapshot_holder().load().version;
    let audit_before = audit_rows(&service).await.len();

    // An override for a subsystem that does not exist.
    let err = service
        .set_sampling_override(
            &ann(),
            SamplingTarget::Subsystem(SubsystemId(99)),
            baseline_override(1),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, fast_proxy_store::StoreError::NotFound(_)),
        "{err}"
    );
    // Settings that break a bound.
    let mut bad = settings(3);
    bad.baseline_per_mille = 5000;
    let err = service.set_sampling(&ann(), bad).await.unwrap_err();
    assert!(
        matches!(&err, fast_proxy_store::StoreError::Invalid(f) if f[0].field == "baselinePerMille")
    );

    assert_eq!(service.snapshot_holder().load().version, version);
    assert_eq!(
        audit_rows(&service).await.len(),
        audit_before,
        "no entry for a vetoed change"
    );
    let (stored, overrides) = service.sampling().await.unwrap();
    assert_eq!(stored.retention_days, 3);
    assert!(overrides.is_empty());
    assert_eq!(route_policy(&service, shop).baseline_per_mille, 1);
}

#[tokio::test]
async fn a_rollback_does_not_change_the_sampling_settings() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    let first = service.snapshot_holder().load().version;
    service.set_sampling(&ann(), settings(9)).await.unwrap();
    service
        .set_sampling_override(
            &ann(),
            SamplingTarget::Subsystem(shop),
            baseline_override(40),
        )
        .await
        .unwrap();
    service.rollback(&ann(), first).await.unwrap();
    let (stored, overrides) = service.sampling().await.unwrap();
    assert_eq!(stored.retention_days, 9);
    assert_eq!(
        overrides.len(),
        1,
        "the subsystem still exists after the rollback"
    );
    assert_eq!(route_policy(&service, shop).baseline_per_mille, 40);
}

#[tokio::test]
async fn a_change_on_one_instance_reaches_the_other_through_the_sync_poll() {
    let cluster = Cluster::new().await;
    let shop = add_site(&cluster.a, "shop").await;
    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert_eq!(route_policy(&cluster.b, shop).baseline_per_mille, 1);

    cluster
        .a
        .set_sampling_override(
            &ann(),
            SamplingTarget::Route(route_id(&cluster.a, shop)),
            baseline_override(250),
        )
        .await
        .unwrap();
    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert_eq!(route_policy(&cluster.b, shop).baseline_per_mille, 250);
    assert!(route_policy(&cluster.b, shop).capture_bodies);

    cluster.a.set_sampling(&ann(), settings(2)).await.unwrap();
    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert_eq!(
        cluster
            .b
            .snapshot_holder()
            .load()
            .sampling_settings
            .retention_days,
        2
    );
}

#[tokio::test]
async fn the_settings_survive_a_restart() {
    let cluster = Cluster::new().await;
    let shop = add_site(&cluster.a, "shop").await;
    cluster.a.set_sampling(&ann(), settings(11)).await.unwrap();
    cluster
        .a
        .set_sampling_override(
            &ann(),
            SamplingTarget::Subsystem(shop),
            baseline_override(33),
        )
        .await
        .unwrap();
    drop(cluster.a);
    drop(cluster.b);
    let restarted = start_at(&cluster.path).await;
    assert_eq!(
        restarted
            .snapshot_holder()
            .load()
            .sampling_settings
            .retention_days,
        11
    );
    assert_eq!(route_policy(&restarted, shop).baseline_per_mille, 33);
}

#[tokio::test]
async fn a_routes_rolling_latency_survives_changes_syncs_and_a_reload() {
    let cluster = Cluster::new().await;
    let shop = add_site(&cluster.a, "shop").await;
    let window = |service: &ConfigService| {
        service.snapshot_holder().load().subsystems[&shop]
            .routes
            .find("/")
            .unwrap()
            .route
            .sampling
            .latency
            .clone()
    };
    let before = window(&cluster.a);
    before.record(120, 1_000);

    cluster.a.set_sampling(&ann(), settings(5)).await.unwrap();
    assert!(
        Arc::ptr_eq(&before, &window(&cluster.a)),
        "a settings change keeps it"
    );
    add_site(&cluster.a, "blog").await;
    assert!(
        Arc::ptr_eq(&before, &window(&cluster.a)),
        "so does an unrelated configuration change"
    );
    cluster.a.reload().await.unwrap();
    assert!(Arc::ptr_eq(&before, &window(&cluster.a)), "and a reload");

    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    let on_b = window(&cluster.b);
    cluster.a.set_sampling(&ann(), settings(6)).await.unwrap();
    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert!(
        Arc::ptr_eq(&on_b, &window(&cluster.b)),
        "a sync reload keeps it too"
    );
}
