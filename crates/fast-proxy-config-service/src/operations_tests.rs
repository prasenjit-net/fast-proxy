//! M5 operations: the actor and audit transaction (CS1), tokens (CS2),
//! maintenance writes and the sweeper (CS3), audit retention (CS4) and reload
//! (CS5).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{
    BypassRules, MaintenanceSettings, MaintenanceTarget, Role, RouteId, SubsystemId, TokenId,
};
use fast_proxy_store::repository::audit::{self, AuditEntry, AuditOutcome, AuditQuery};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{revision, Connection, Mutation, Store, StoreError};
use fast_proxy_telemetry::SyncResult;
use tempfile::TempDir;

use crate::tests::create_site;
use crate::{AuditRetention, Audited, Bootstrap, ChangeContext, ConfigService, StartError};

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
    a: Arc<ConfigService>,
    b: Arc<ConfigService>,
}

impl Cluster {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let a = start_at(&path).await;
        let b = start_at(&path).await;
        Self { _dir: dir, a, b }
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

async fn audit_rows(service: &ConfigService, query: AuditQuery) -> Vec<AuditEntry> {
    service
        .read(move |conn| audit::list(conn, &query, None, 100))
        .await
        .unwrap()
        .value
}

async fn all_audit(service: &ConfigService) -> Vec<AuditEntry> {
    audit_rows(service, AuditQuery::default()).await
}

fn on(message: &str, expires_at_ms: Option<i64>) -> MaintenanceSettings {
    MaintenanceSettings {
        message: message.into(),
        body: None,
        retry_after_secs: None,
        bypass: BypassRules::default(),
        expires_at_ms,
        set_by: String::new(),
        set_at_ms: 0,
    }
}

fn now() -> i64 {
    crate::service::now_ms()
}

// ---- CS1: the actor and the audit transaction ----

#[tokio::test]
async fn a_committed_change_has_one_audit_entry_and_names_its_actor_on_the_revision() {
    let service = memory().await;
    add_site(&service, "shop").await;

    let rows = all_audit(&service).await;
    assert_eq!(rows.len(), 1);
    let entry = &rows[0].entry;
    assert_eq!(entry.actor, "ann");
    assert_eq!(entry.role, "admin");
    assert_eq!(entry.source, "10.0.0.5");
    assert_eq!(entry.request_id, "req-1");
    assert_eq!(entry.action, "subsystem.create");
    assert_eq!(entry.target, "shop");
    assert_eq!(entry.outcome, AuditOutcome::Ok);
    assert!(!entry.instance.is_empty());

    let revision_number = entry.revision.unwrap();
    assert_eq!(revision_number, service.snapshot_holder().load().version);
    let revision = service
        .read(move |conn| revision::get(conn, revision_number))
        .await
        .unwrap()
        .value;
    assert_eq!(revision.actor, "ann");
}

#[tokio::test]
async fn a_vetoed_or_failed_change_leaves_neither_an_entry_nor_a_revision() {
    let service = memory().await;
    add_site(&service, "shop").await;
    let before = service.snapshot_holder().load().version;

    // A duplicate name fails inside the transaction.
    let duplicate = service
        .apply(
            &ann(),
            Audited::new("subsystem.create", "shop"),
            create_site("shop", "other.example.com".into(), "127.0.0.1:3001"),
        )
        .await;
    assert!(duplicate.is_err());

    assert_eq!(all_audit(&service).await.len(), 1, "only the first change");
    assert_eq!(service.snapshot_holder().load().version, before);
}

#[tokio::test]
async fn concurrent_writes_keep_their_own_actors() {
    let service = memory().await;
    let bob = ChangeContext::token("bob", "admin", "10.0.0.6", "req-2");
    let ann = ann();
    let (first, second) = tokio::join!(
        service.apply(
            &ann,
            Audited::new("subsystem.create", "one"),
            create_site("one", "one.example.com".into(), "127.0.0.1:3000"),
        ),
        service.apply(
            &bob,
            Audited::new("subsystem.create", "two"),
            create_site("two", "two.example.com".into(), "127.0.0.1:3001"),
        ),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    for (applied, actor, target) in [(&first, "ann", "one"), (&second, "bob", "two")] {
        let version = applied.version;
        let revision = service
            .read(move |conn| revision::get(conn, version))
            .await
            .unwrap()
            .value;
        assert_eq!(revision.actor, actor);
        let entries = audit_rows(
            &service,
            AuditQuery {
                actor: Some(actor.into()),
                ..AuditQuery::default()
            },
        )
        .await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.target, target);
        assert_eq!(entries[0].entry.revision, Some(version));
    }
}

#[tokio::test]
async fn a_rollback_is_attributed_and_audited() {
    let service = memory().await;
    add_site(&service, "shop").await;
    service.rollback(&ann(), 1).await.unwrap();
    let rows = audit_rows(
        &service,
        AuditQuery {
            action: Some("revision.rollback".into()),
            ..AuditQuery::default()
        },
    )
    .await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry.target, "v1");
}

// ---- CS2: tokens ----

#[tokio::test]
async fn a_created_token_authenticates_until_it_is_revoked() {
    let service = memory().await;
    let admin = service
        .create_token(&ann(), "root", Role::Admin, None)
        .await
        .unwrap()
        .value;
    let viewer = service
        .create_token(&ann(), "eyes", Role::Viewer, None)
        .await
        .unwrap()
        .value;
    let cache = service.token_cache();
    assert_eq!(
        cache
            .authenticate(viewer.token.reveal(), now())
            .unwrap()
            .role,
        Role::Viewer
    );

    service.revoke_token(&ann(), viewer.id).await.unwrap();
    assert!(cache.authenticate(viewer.token.reveal(), now()).is_err());
    assert!(cache.authenticate(admin.token.reveal(), now()).is_ok());
}

#[tokio::test]
async fn the_last_admin_token_cannot_be_revoked() {
    let service = memory().await;
    let only = service
        .create_token(&ann(), "root", Role::Admin, None)
        .await
        .unwrap()
        .value;
    let err = service.revoke_token(&ann(), only.id).await.unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    assert!(service
        .token_cache()
        .authenticate(only.token.reveal(), now())
        .is_ok());
}

#[tokio::test]
async fn a_token_expires_without_any_write() {
    let service = memory().await;
    let short = service
        .create_token(&ann(), "short", Role::Viewer, Some(now() + 1_000))
        .await
        .unwrap()
        .value;
    let cache = service.token_cache();
    assert!(cache.authenticate(short.token.reveal(), now()).is_ok());
    assert!(cache
        .authenticate(short.token.reveal(), now() + 2_000)
        .is_err());
}

#[tokio::test]
async fn token_changes_are_audited_without_any_secret() {
    let service = memory().await;
    let created = service
        .create_token(&ann(), "root", Role::Admin, None)
        .await
        .unwrap()
        .value;
    service
        .create_token(&ann(), "second", Role::Admin, None)
        .await
        .unwrap();
    service.revoke_token(&ann(), created.id).await.unwrap();

    let actions: Vec<String> = all_audit(&service)
        .await
        .into_iter()
        .map(|row| row.entry.action)
        .collect();
    assert_eq!(actions, ["token.revoke", "token.create", "token.create"]);

    let secret = created
        .token
        .reveal()
        .rsplit('.')
        .next()
        .unwrap()
        .to_string();
    let stored: Vec<String> = service
        .read(|conn| {
            let mut statement = conn.prepare(
                "SELECT detail || target || actor FROM audit_log
                 UNION ALL SELECT summary || config || actor FROM revision",
            )?;
            let rows = statement.query_map([], |row| row.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
        .unwrap()
        .value;
    assert!(!stored.is_empty());
    assert!(stored.iter().all(|text| !text.contains(&secret)));
}

#[tokio::test]
async fn bootstrap_creates_one_admin_token_only_while_none_exists() {
    let service = memory().await;
    let Bootstrap::Created(token) = service.ensure_admin_token(None).await.unwrap() else {
        panic!("a fresh database needs a token");
    };
    assert!(service
        .token_cache()
        .authenticate(token.reveal(), now())
        .is_ok());
    assert!(matches!(
        service.ensure_admin_token(None).await.unwrap(),
        Bootstrap::NotNeeded
    ));
    assert_eq!(service.list_tokens().await.unwrap().len(), 1);
    let rows = all_audit(&service).await;
    assert_eq!(rows[0].entry.action, "token.bootstrap");
    assert_eq!(rows[0].entry.actor, "bootstrap");
}

#[tokio::test]
async fn a_seeded_bootstrap_token_is_the_operators_own() {
    let service = memory().await;
    let seed = format!("fp_7.{}", "s".repeat(40));
    assert!(matches!(
        service.ensure_admin_token(Some(&seed)).await.unwrap(),
        Bootstrap::Seeded
    ));
    let principal = service.token_cache().authenticate(&seed, now()).unwrap();
    assert_eq!(principal.token_id, TokenId(7));
    assert_eq!(principal.role, Role::Admin);
}

#[tokio::test]
async fn a_weak_or_malformed_seed_is_refused_and_stores_nothing() {
    let service = memory().await;
    for bad in ["nonsense", "fp_1.short", "fp_x.".to_string().as_str()] {
        let err = service.ensure_admin_token(Some(bad)).await.unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid(f) if f[0].field == "FAST_PROXY_BOOTSTRAP_TOKEN"),
            "{bad}"
        );
    }
    assert!(service.list_tokens().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_later_bootstrap_after_all_admins_are_gone_picks_a_free_name() {
    let service = memory().await;
    service.ensure_admin_token(None).await.unwrap();
    let revoked_at = now();
    service
        .read(move |conn| {
            conn.execute("UPDATE admin_token SET revoked_at = ?1", [revoked_at])
                .map_err(StoreError::from)
        })
        .await
        .unwrap();
    assert!(matches!(
        service.ensure_admin_token(None).await.unwrap(),
        Bootstrap::Created(_)
    ));
    let names: Vec<String> = service
        .list_tokens()
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.name.as_str().to_string())
        .collect();
    assert_eq!(names, ["bootstrap", "bootstrap-2"]);
}

#[tokio::test]
async fn two_services_on_one_database_see_a_new_token_and_a_revocation() {
    let cluster = Cluster::new().await;
    let root = cluster
        .a
        .create_token(&ann(), "root", Role::Admin, None)
        .await
        .unwrap()
        .value;
    let extra = cluster
        .a
        .create_token(&ann(), "extra", Role::Viewer, None)
        .await
        .unwrap()
        .value;
    let b_cache = cluster.b.token_cache();
    assert!(b_cache.authenticate(extra.token.reveal(), now()).is_err());

    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert!(b_cache.authenticate(extra.token.reveal(), now()).is_ok());

    cluster.a.revoke_token(&ann(), extra.id).await.unwrap();
    assert_eq!(cluster.b.sync_once().await, SyncResult::Reloaded);
    assert!(b_cache.authenticate(extra.token.reveal(), now()).is_err());
    assert!(b_cache.authenticate(root.token.reveal(), now()).is_ok());
}

#[tokio::test]
async fn several_instances_bootstrapping_together_create_one_token() {
    let cluster = Cluster::new().await;
    let (a, b) = tokio::join!(
        cluster.a.ensure_admin_token(None),
        cluster.b.ensure_admin_token(None)
    );
    let created = [a.unwrap(), b.unwrap()]
        .into_iter()
        .filter(|outcome| matches!(outcome, Bootstrap::Created(_)))
        .count();
    assert_eq!(created, 1);
    assert_eq!(cluster.a.list_tokens().await.unwrap().len(), 1);
}

// ---- CS3: maintenance ----

#[tokio::test]
async fn every_scope_can_be_set_replaced_and_cleared_with_audit_entries() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    // The first route created in a fresh database.
    let route = RouteId(1);
    let targets = [
        MaintenanceTarget::Global,
        MaintenanceTarget::Subsystem(shop),
        MaintenanceTarget::Route(route),
    ];
    for target in targets {
        service
            .set_maintenance(&ann(), target, on("deploying", None))
            .await
            .unwrap();
    }
    service
        .set_maintenance(&ann(), MaintenanceTarget::Global, on("changed", None))
        .await
        .unwrap();

    let rows = service.list_maintenance().await.unwrap();
    assert_eq!(rows.len(), 3);
    let global = rows
        .iter()
        .find(|row| row.target == MaintenanceTarget::Global)
        .unwrap();
    assert_eq!(global.settings.message, "changed");
    assert_eq!(
        global.settings.set_by, "ann",
        "from the context, not the input"
    );
    assert!(global.settings.set_at_ms > 0);

    for target in targets {
        assert!(
            service
                .clear_maintenance(&ann(), target)
                .await
                .unwrap()
                .value
        );
    }
    assert!(service.list_maintenance().await.unwrap().is_empty());

    let sets = audit_rows(
        &service,
        AuditQuery {
            action: Some("maintenance.set".into()),
            ..AuditQuery::default()
        },
    )
    .await;
    assert_eq!(sets.len(), 4);
}

#[tokio::test]
async fn clearing_what_is_not_on_writes_nothing() {
    let service = memory().await;
    let before = service.snapshot_holder().load().version;
    let outcome = service
        .clear_maintenance(&ann(), MaintenanceTarget::Global)
        .await
        .unwrap();
    assert!(!outcome.value);
    assert_eq!(outcome.version, before);
    assert!(all_audit(&service).await.is_empty());
}

#[tokio::test]
async fn invalid_settings_are_refused_and_change_nothing() {
    let service = memory().await;
    let mut bad = on("x", None);
    bad.retry_after_secs = Some(0);
    let err = service
        .set_maintenance(&ann(), MaintenanceTarget::Global, bad)
        .await
        .unwrap_err();
    assert!(matches!(&err, StoreError::Invalid(f) if f[0].field == "retryAfterSecs"));
    assert!(service.list_maintenance().await.unwrap().is_empty());
    assert!(all_audit(&service).await.is_empty());
}

#[tokio::test]
async fn deleting_a_subsystem_removes_its_scope() {
    let service = memory().await;
    let shop = add_site(&service, "shop").await;
    service
        .set_maintenance(&ann(), MaintenanceTarget::Subsystem(shop), on("x", None))
        .await
        .unwrap();
    service
        .apply(
            &ann(),
            Audited::new("subsystem.delete", "shop"),
            move |conn| {
                subsystems::delete(conn, shop)?;
                Ok(Mutation {
                    value: (),
                    summary: "deleted".into(),
                })
            },
        )
        .await
        .unwrap();
    assert!(service.list_maintenance().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rollback_does_not_switch_maintenance_on_or_off() {
    let service = memory().await;
    add_site(&service, "shop").await;
    service
        .set_maintenance(&ann(), MaintenanceTarget::Global, on("x", None))
        .await
        .unwrap();
    service.rollback(&ann(), 1).await.unwrap();
    assert_eq!(service.list_maintenance().await.unwrap().len(), 1);
}

#[tokio::test]
async fn maintenance_and_its_expiry_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    let expires = now() + 600_000;
    {
        let service = start_at(&path).await;
        service
            .set_maintenance(&ann(), MaintenanceTarget::Global, on("x", Some(expires)))
            .await
            .unwrap();
    }
    let restarted = start_at(&path).await;
    let rows = restarted.list_maintenance().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].settings.expires_at_ms, Some(expires));
}

#[tokio::test]
async fn the_sweeper_clears_only_expired_scopes_and_audits_each_expiry_once() {
    let cluster = Cluster::new().await;
    cluster
        .a
        .set_maintenance(
            &ann(),
            MaintenanceTarget::Global,
            on("short", Some(now() + 50)),
        )
        .await
        .unwrap();
    let shop = add_site(&cluster.a, "shop").await;
    cluster
        .a
        .set_maintenance(
            &ann(),
            MaintenanceTarget::Subsystem(shop),
            on("long", Some(now() + 600_000)),
        )
        .await
        .unwrap();

    assert_eq!(
        cluster.a.sweep_expired_maintenance().await.unwrap(),
        0,
        "not yet"
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(cluster.a.sweep_expired_maintenance().await.unwrap(), 1);
    assert_eq!(
        cluster.b.sweep_expired_maintenance().await.unwrap(),
        0,
        "the other instance finds it already gone"
    );

    let rows = cluster.a.list_maintenance().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target, MaintenanceTarget::Subsystem(shop));

    let expired = audit_rows(
        &cluster.a,
        AuditQuery {
            action: Some("maintenance.expired".into()),
            ..AuditQuery::default()
        },
    )
    .await;
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].entry.actor, "system");
    assert_eq!(expired[0].entry.target, "global");
}

// ---- CS4: audit retention ----

async fn append_entries(service: &ConfigService, count: i64, at_ms: i64) {
    service
        .read(move |conn| {
            for n in 0..count {
                audit::append(
                    conn,
                    &audit::NewAuditEntry {
                        at_ms: at_ms + n,
                        instance: "i".into(),
                        actor: "ann".into(),
                        role: "admin".into(),
                        source: "s".into(),
                        request_id: "r".into(),
                        action: "x.y".into(),
                        target: "t".into(),
                        outcome: AuditOutcome::Ok,
                        detail: "{}".into(),
                        revision: None,
                    },
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn pruning_removes_entries_past_the_age_limit() {
    let service = memory().await;
    let day = 86_400_000;
    append_entries(&service, 3, 0).await;
    append_entries(&service, 2, 100 * day).await;
    let retention = AuditRetention {
        max_age: Duration::from_secs(90 * 86_400),
        max_rows: 1000,
    };
    let removed = service.prune_audit_log(retention, 101 * day).await.unwrap();
    assert_eq!(removed, 3);
    assert_eq!(all_audit(&service).await.len(), 2);
}

#[tokio::test]
async fn pruning_by_count_keeps_the_newest_entries() {
    let service = memory().await;
    append_entries(&service, 2_500, 1_000_000).await;
    let retention = AuditRetention {
        max_age: Duration::from_secs(86_400),
        max_rows: 100,
    };
    let removed = service.prune_audit_log(retention, 1_000_000).await.unwrap();
    assert_eq!(removed, 2_400, "several batches");
    let remaining = all_audit(&service).await;
    assert_eq!(remaining.len(), 100);
    assert_eq!(
        remaining[0].entry.at_ms,
        1_000_000 + 2_499,
        "the newest survives"
    );
}

// ---- CS5: reload ----

fn edit_behind_the_services_back(path: &Path, sql: &str) {
    let conn = Connection::open(path).unwrap();
    conn.execute(sql, []).unwrap();
}

#[tokio::test]
async fn reload_picks_up_a_database_edit_that_did_not_move_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    let service = start_at(&path).await;
    add_site(&service, "shop").await;
    let live = service.snapshot_holder().load().version;

    edit_behind_the_services_back(
        &path,
        "INSERT INTO subsystem (name, created_at, updated_at) VALUES ('hand-made', 1, 1)",
    );
    assert_eq!(
        service.sync_once().await,
        SyncResult::Unchanged,
        "same version"
    );
    assert_eq!(service.snapshot_holder().load().subsystems.len(), 1);

    let version = service.reload().await.unwrap();
    assert_eq!(version, live);
    assert_eq!(service.snapshot_holder().load().subsystems.len(), 2);
}

#[tokio::test]
async fn a_failing_reload_keeps_the_old_snapshot_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    let service = start_at(&path).await;
    add_site(&service, "shop").await;

    edit_behind_the_services_back(
        &path,
        "INSERT INTO subsystem_host (subsystem_id, pattern) VALUES (1, 'not a host!')",
    );
    let err = service.reload().await.unwrap_err();
    assert!(matches!(err, StartError::Store(_)));
    assert_eq!(service.snapshot_holder().load().subsystems.len(), 1);
}

#[tokio::test]
async fn reload_announces_itself_to_subscribers_and_refreshes_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    let service = start_at(&path).await;
    let mut changes = service.subscribe();
    let seed = format!("fp_3.{}", "k".repeat(40));
    edit_behind_the_services_back(
        &path,
        &format!(
            "INSERT INTO admin_token (id, name, role, secret_hash, created_at, created_by)
             VALUES (3, 'by-hand', 'admin', X'{}', 1, 'cli')",
            hex(fast_proxy_auth::hash_secret(&"k".repeat(40)).as_bytes())
        ),
    );
    assert!(service.token_cache().authenticate(&seed, now()).is_err());
    service.reload().await.unwrap();
    assert!(service.token_cache().authenticate(&seed, now()).is_ok());
    assert!(changes.try_recv().is_ok());
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn the_sweeper_and_pruner_loops_stop_on_shutdown_and_a_zero_interval_disables_them() {
    let service = memory().await;
    let stop = tokio_util::sync::CancellationToken::new();
    service
        .clone()
        .run_maintenance_sweeper(Duration::ZERO, stop.clone())
        .await;
    service
        .clone()
        .run_audit_pruner(
            AuditRetention {
                max_age: Duration::from_secs(1),
                max_rows: 1,
            },
            Duration::ZERO,
            stop.clone(),
        )
        .await;

    let sweeper = tokio::spawn(
        service
            .clone()
            .run_maintenance_sweeper(Duration::from_millis(10), stop.clone()),
    );
    let pruner = tokio::spawn(service.clone().run_audit_pruner(
        AuditRetention {
            max_age: Duration::from_secs(1),
            max_rows: 1,
        },
        Duration::from_millis(10),
        stop.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), async {
        sweeper.await.unwrap();
        pruner.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn the_running_sweeper_clears_an_expired_scope_by_itself() {
    let service = memory().await;
    service
        .set_maintenance(
            &ann(),
            MaintenanceTarget::Global,
            on("short", Some(now() + 30)),
        )
        .await
        .unwrap();
    let stop = tokio_util::sync::CancellationToken::new();
    tokio::spawn(
        service
            .clone()
            .run_maintenance_sweeper(Duration::from_millis(10), stop.clone()),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !service.list_maintenance().await.unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the sweeper clears it");
    stop.cancel();
}
