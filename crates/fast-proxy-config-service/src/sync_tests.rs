//! The cluster sync: two config services on one database file, as two
//! instances (or processes) sharing it would be.

use crate::{Audited, ChangeContext};
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{CertificateId, HostPattern, HttpsPolicy, SubsystemId};
use fast_proxy_routing::SniLookup;
use fast_proxy_store::repository::hosts;
use fast_proxy_store::{Mutation, Store};
use fast_proxy_telemetry::{Metrics, SyncResult};
use fast_proxy_test_pki::{LeafSpec, TestCa};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use crate::tests::create_site;
use crate::{CertificateChange, ConfigService, TlsSettings, VaultStatus};

const KEY_ONE: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
const KEY_TWO: &str = "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=";

struct Pair {
    dir: TempDir,
    a: Arc<ConfigService>,
    b: Arc<ConfigService>,
    b_metrics: Arc<Metrics>,
}

fn settings(key: &str) -> TlsSettings {
    TlsSettings {
        enabled: true,
        master_key_file: None,
        env_master_key: Some(key.into()),
    }
}

async fn start(dir: &TempDir, key: &str) -> (Arc<ConfigService>, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::new());
    let store = Store::open(&dir.path().join("shared.db")).unwrap();
    let service = ConfigService::start_with_tls(store, metrics.clone(), settings(key))
        .await
        .unwrap();
    (Arc::new(service), metrics)
}

impl Pair {
    async fn new() -> Self {
        Self::with_keys(KEY_ONE, KEY_ONE).await
    }

    async fn with_keys(a_key: &str, b_key: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = start(&dir, a_key).await;
        let (b, b_metrics) = start(&dir, b_key).await;
        Self {
            dir,
            a,
            b,
            b_metrics,
        }
    }

    async fn add_shop(&self) -> SubsystemId {
        self.a
            .apply(
                &ChangeContext::system(),
                Audited::new("test.change", "test"),
                create_site("shop", "shop.example.com".into(), "127.0.0.1:3000"),
            )
            .await
            .unwrap()
            .value
    }
}

fn certificate(service: &ConfigService, name: &str) -> Option<Arc<fast_proxy_tls::CertHandle>> {
    let snapshot = service.snapshot_holder().load();
    match snapshot.sni_lookup(name) {
        SniLookup::Subsystem { https, .. } => https.certificate.clone(),
        SniLookup::Unknown => None,
    }
}

fn https_on(certificate: Option<CertificateId>) -> HttpsPolicy {
    HttpsPolicy {
        enabled: true,
        certificate_id: certificate,
        ..HttpsPolicy::default()
    }
}

fn version(service: &ConfigService) -> u64 {
    service.snapshot_holder().load().version
}

#[tokio::test]
async fn a_subsystem_edit_on_one_instance_reaches_the_other() {
    let pair = Pair::new().await;
    assert!(pair.b.snapshot_holder().load().subsystems.is_empty());
    pair.add_shop().await;
    assert_eq!(pair.b.sync_once().await, SyncResult::Reloaded);
    assert_eq!(pair.b.snapshot_holder().load().subsystems.len(), 1);
    assert_eq!(version(&pair.b), version(&pair.a));
    let mut changes = pair.b.subscribe();
    pair.a
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            |conn| {
                hosts::add(
                    conn,
                    SubsystemId(1),
                    &HostPattern::parse("extra.example.com").unwrap(),
                )?;
                Ok(Mutation {
                    value: (),
                    summary: "host added".into(),
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(pair.b.sync_once().await, SyncResult::Reloaded);
    assert!(
        changes.try_recv().is_ok(),
        "the reload is announced like any change"
    );
    assert!(pair
        .b
        .snapshot_holder()
        .load()
        .hosts
        .lookup("extra.example.com")
        .is_some());
}

#[tokio::test]
async fn certificate_uploads_renewals_https_toggles_and_deletions_all_propagate() {
    let pair = Pair::new().await;
    let site = pair.add_shop().await;
    let ca = TestCa::new("ca");
    let issued = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let id = pair
        .a
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &issued.chain_pem,
            &issued.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    pair.a
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    pair.b.sync_once().await;
    let first = certificate(&pair.b, "shop.example.com").expect("upload and attach arrived");

    let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    pair.a
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                cert_pem: Some(renewed.chain_pem),
                key_pem: Some(renewed.key_pem),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    pair.b.sync_once().await;
    let second = certificate(&pair.b, "shop.example.com").unwrap();
    assert_ne!(first.version, second.version, "the renewal arrived");

    pair.a
        .set_subsystem_https(&ChangeContext::system(), site, HttpsPolicy::default())
        .await
        .unwrap();
    pair.b.sync_once().await;
    let snapshot = pair.b.snapshot_holder().load();
    assert!(!snapshot.subsystems.values().next().unwrap().https.enabled);
    drop(snapshot);

    pair.a
        .delete_certificate(&ChangeContext::system(), id)
        .await
        .unwrap();
    pair.b.sync_once().await;
    assert!(pair.b.list_certificates().await.unwrap().is_empty());
}

#[tokio::test]
async fn the_receiver_reuses_unchanged_certificates_and_endpoint_runtime_state() {
    let pair = Pair::new().await;
    let site = pair.add_shop().await;
    let ca = TestCa::new("ca");
    let one = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    let id = pair
        .a
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &one.chain_pem,
            &one.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    let other = ca.issue(&LeafSpec::new(&["blog.example.com"]));
    pair.a
        .upload_certificate(
            &ChangeContext::system(),
            "blog",
            &other.chain_pem,
            &other.key_pem,
        )
        .await
        .unwrap();
    pair.a
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    pair.b.sync_once().await;
    let handle = certificate(&pair.b, "shop.example.com").unwrap();
    let runtime = {
        let snapshot = pair.b.snapshot_holder().load();
        let upstream = snapshot
            .subsystems
            .values()
            .next()
            .unwrap()
            .upstreams
            .values()
            .next()
            .unwrap()
            .clone();
        Arc::clone(&upstream.candidates[0].runtime)
    };

    // An unrelated certificate changes on the other instance.
    let renewed = ca.issue(&LeafSpec::new(&["blog.example.com"]));
    let blog = pair
        .a
        .list_certificates()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.name.as_str() == "blog")
        .unwrap();
    pair.a
        .replace_certificate(
            &ChangeContext::system(),
            blog.id,
            CertificateChange {
                cert_pem: Some(renewed.chain_pem),
                key_pem: Some(renewed.key_pem),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(pair.b.sync_once().await, SyncResult::Reloaded);
    assert!(Arc::ptr_eq(
        &handle,
        &certificate(&pair.b, "shop.example.com").unwrap()
    ));
    let after = pair.b.snapshot_holder().load();
    let upstream = after
        .subsystems
        .values()
        .next()
        .unwrap()
        .upstreams
        .values()
        .next()
        .unwrap();
    assert!(
        Arc::ptr_eq(&runtime, &upstream.candidates[0].runtime),
        "endpoint state survives a reload"
    );
}

#[tokio::test]
async fn nothing_reloads_when_nothing_changed_and_a_local_commit_never_triggers_one() {
    let pair = Pair::new().await;
    pair.add_shop().await;
    assert_eq!(
        pair.a.sync_once().await,
        SyncResult::Unchanged,
        "own commit"
    );
    let before = pair.b.snapshot_holder().load().version;
    assert_eq!(pair.b.sync_once().await, SyncResult::Reloaded);
    assert_eq!(pair.b.sync_once().await, SyncResult::Unchanged);
    assert!(pair.b.snapshot_holder().load().version > before);
    let text = pair.b_metrics.render();
    assert!(
        text.contains("fastproxy_config_sync_total{result=\"reloaded\"} 1"),
        "{text}"
    );
    assert!(
        text.contains("fastproxy_config_sync_total{result=\"unchanged\"} 1"),
        "{text}"
    );
}

#[tokio::test]
async fn a_stale_instance_never_writes_over_a_newer_database_state() {
    let pair = Pair::new().await;
    pair.add_shop().await;
    pair.b.sync_once().await;
    pair.a
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            create_site("blog", "blog.example.com".into(), "127.0.0.1:3001"),
        )
        .await
        .unwrap();
    // B has not synced, and writes.
    pair.b
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            create_site("docs", "docs.example.com".into(), "127.0.0.1:3002"),
        )
        .await
        .unwrap();
    let names = |service: &ConfigService| {
        let mut names: Vec<String> = service
            .snapshot_holder()
            .load()
            .subsystems
            .values()
            .map(|s| s.name.as_str().to_string())
            .collect();
        names.sort();
        names
    };
    assert_eq!(
        names(&pair.b),
        ["blog", "docs", "shop"],
        "B's write is built on the real state"
    );
    assert_eq!(pair.a.sync_once().await, SyncResult::Reloaded);
    assert_eq!(names(&pair.a), ["blog", "docs", "shop"]);
    assert_eq!(version(&pair.a), version(&pair.b));
}

#[tokio::test]
async fn a_different_master_key_is_reported_and_its_certificates_are_not_served() {
    let pair = Pair::with_keys(KEY_ONE, KEY_TWO).await;
    let site = pair.add_shop().await;
    pair.a
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    assert_eq!(
        pair.b.vault_status(),
        VaultStatus::Ok,
        "nothing sealed yet to disagree with"
    );
    pair.b.sync_once().await;
    assert_eq!(pair.b.vault_status(), VaultStatus::KeyMismatch);
    assert!(
        certificate(&pair.b, "shop.example.com").is_none(),
        "unreadable, not served"
    );
    let listed = pair.b.list_certificates().await.unwrap();
    assert_eq!(listed[0].state.as_str(), "unreadable");
    assert!(certificate(&pair.a, "shop.example.com").is_some());
}

#[tokio::test]
async fn a_failing_reload_is_counted_keeps_the_old_snapshot_and_is_retried() {
    let pair = Pair::new().await;
    pair.add_shop().await;
    pair.b.sync_once().await;
    let good = version(&pair.b);
    // Another process leaves HTTPS enabled with no certificate and TLS on: it cannot compile.
    {
        let store = Store::open(&pair.dir.path().join("shared.db")).unwrap();
        store
            .run(|db| {
                db.write(
                    |conn| {
                        conn.execute("UPDATE subsystem SET https_enabled = 1", [])?;
                        Ok(Mutation {
                            value: (),
                            summary: "broken by hand".into(),
                        })
                    },
                    |_| Ok(()),
                )
            })
            .await
            .unwrap();
    }
    assert_eq!(pair.b.sync_once().await, SyncResult::Failed);
    assert_eq!(version(&pair.b), good, "the old snapshot keeps serving");
    assert_eq!(
        pair.b.sync_once().await,
        SyncResult::Failed,
        "retried on the next poll"
    );
    let text = pair.b_metrics.render();
    assert!(
        text.contains("fastproxy_config_sync_total{result=\"failed\"} 2"),
        "{text}"
    );
}

#[tokio::test]
async fn the_poll_task_applies_changes_within_the_interval_and_stops_on_shutdown() {
    let pair = Pair::new().await;
    let stop = CancellationToken::new();
    let task = tokio::spawn(
        pair.b
            .clone()
            .run_sync(Duration::from_millis(20), stop.clone()),
    );
    pair.add_shop().await;
    let mut seen = false;
    for _ in 0..200 {
        if pair.b.snapshot_holder().load().subsystems.len() == 1 {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(seen, "the change arrived without an explicit sync");
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_zero_interval_does_nothing() {
    let pair = Pair::new().await;
    let task = tokio::spawn(
        pair.b
            .clone()
            .run_sync(Duration::ZERO, CancellationToken::new()),
    );
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    pair.add_shop().await;
    assert!(pair.b.snapshot_holder().load().subsystems.is_empty());
}
