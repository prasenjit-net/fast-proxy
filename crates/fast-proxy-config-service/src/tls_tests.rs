//! Certificate writes, the certificate guarantee, startup and key protection,
//! against a real database file and master key file.

use crate::{Audited, ChangeContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fast_proxy_model::{CertificateId, CertificateSource, HostPattern, HttpsPolicy, SubsystemId};
use fast_proxy_routing::SniLookup;
use fast_proxy_store::repository::{hosts, subsystems};
use fast_proxy_store::{Mutation, Store, StoreError, SubsystemFields};
use fast_proxy_telemetry::Metrics;
use fast_proxy_test_pki::{LeafSpec, TestCa};
use tempfile::TempDir;

use crate::tests::create_site;
use crate::{CertificateChange, ConfigService, TlsSettings, VaultStatus};

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn database(&self) -> PathBuf {
        self.dir.path().join("fast-proxy.db")
    }

    fn key_file(&self) -> PathBuf {
        self.dir.path().join("master.key")
    }

    fn settings(&self, enabled: bool) -> TlsSettings {
        TlsSettings {
            enabled,
            master_key_file: Some(self.key_file()),
            env_master_key: None,
        }
    }

    async fn start(&self, settings: TlsSettings) -> ConfigService {
        let store = Store::open(&self.database()).unwrap();
        ConfigService::start_with_tls(store, Arc::new(Metrics::new()), settings)
            .await
            .unwrap()
    }
}

async fn add_shop(service: &ConfigService) -> SubsystemId {
    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            create_site("shop", "shop.example.com".into(), "127.0.0.1:3000"),
        )
        .await
        .unwrap()
        .value
}

fn https_on(certificate: Option<CertificateId>) -> HttpsPolicy {
    HttpsPolicy {
        enabled: true,
        certificate_id: certificate,
        ..HttpsPolicy::default()
    }
}

fn certificate_for(service: &ConfigService, name: &str) -> Option<Arc<fast_proxy_tls::CertHandle>> {
    let snapshot = service.snapshot_holder().load();
    match snapshot.sni_lookup(name) {
        SniLookup::Subsystem { https, .. } => https.certificate.clone(),
        SniLookup::Unknown => None,
    }
}

async fn names_of(service: &ConfigService) -> Vec<String> {
    service
        .list_certificates()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name.as_str().to_string())
        .collect()
}

fn leaf(ca: &TestCa, names: &[&str]) -> fast_proxy_test_pki::Issued {
    ca.issue(&LeafSpec::new(names))
}

#[tokio::test]
async fn enabling_https_with_tls_on_generates_and_attaches_a_dev_certificate_in_the_same_write() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let before = service.snapshot_holder().load().version;

    let applied = service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    assert_eq!(applied.version, before + 1, "one write, one revision");

    let handle = certificate_for(&service, "shop.example.com").expect("a certificate is attached");
    assert_eq!(handle.name.as_str(), "dev-shop");
    assert_eq!(handle.source, CertificateSource::SelfSigned);
    let listed = service.list_certificates().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].used_by, vec![site]);
    assert!(service.snapshot_holder().load().tls_enabled);
}

#[tokio::test]
async fn with_tls_off_nothing_is_generated_and_no_master_key_is_created() {
    let env = Env::new();
    let service = env.start(env.settings(false)).await;
    let site = add_shop(&service).await;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    assert!(service.list_certificates().await.unwrap().is_empty());
    assert!(certificate_for(&service, "shop.example.com").is_none());
    assert!(!env.key_file().exists());
    assert_eq!(service.vault_status(), VaultStatus::NotCreated);
    assert!(!service.snapshot_holder().load().tls_enabled);
}

#[tokio::test]
async fn the_master_key_is_created_once_only_when_a_key_must_be_sealed() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    add_shop(&service).await;
    assert!(!env.key_file().exists(), "TLS on alone seals nothing");

    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    service
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &issued.chain_pem,
            &issued.key_pem,
        )
        .await
        .unwrap();
    let key_bytes = std::fs::read(env.key_file()).unwrap();
    service
        .generate_self_signed(
            &ChangeContext::system(),
            "scratch",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(env.key_file()).unwrap(),
        key_bytes,
        "reused, not rewritten"
    );
    assert_eq!(service.vault_status(), VaultStatus::Ok);

    drop(service);
    let restarted = env.start(env.settings(true)).await;
    assert_eq!(restarted.vault_status(), VaultStatus::Ok);
    let states: Vec<_> = restarted
        .list_certificates()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.state.as_str())
        .collect();
    assert_eq!(
        states,
        ["ok", "ok"],
        "the certificates survive a restart and stay readable"
    );
}

#[tokio::test]
async fn an_uploaded_certificate_is_attached_served_and_never_replaced() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com", "*.shop.example.com"]);
    let uploaded = service
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &issued.chain_pem,
            &issued.key_pem,
        )
        .await
        .unwrap()
        .value;
    let applied = service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(uploaded.id)))
        .await
        .unwrap();
    assert!(applied.value.is_empty(), "no uncovered host");
    let handle = certificate_for(&service, "shop.example.com").unwrap();
    assert_eq!(handle.source, CertificateSource::Uploaded);

    // A host change must not touch an uploaded certificate.
    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            move |conn| {
                hosts::add(
                    conn,
                    site,
                    &HostPattern::parse("other.example.com").unwrap(),
                )?;
                Ok(Mutation {
                    value: (),
                    summary: "host added".into(),
                })
            },
        )
        .await
        .unwrap();
    let after = certificate_for(&service, "shop.example.com").unwrap();
    assert!(Arc::ptr_eq(&handle, &after));
    assert_eq!(names_of(&service).await, ["shop"]);
}

#[tokio::test]
async fn adding_a_host_regenerates_the_dev_certificate_in_the_same_write() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    let first = certificate_for(&service, "shop.example.com").unwrap();
    assert!(certificate_for(&service, "new.example.com").is_none());

    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            move |conn| {
                hosts::add(conn, site, &HostPattern::parse("new.example.com").unwrap())?;
                Ok(Mutation {
                    value: (),
                    summary: "host added".into(),
                })
            },
        )
        .await
        .unwrap();
    let second = certificate_for(&service, "new.example.com").expect("covers the new host");
    assert_eq!(second.id, first.id, "replaced in place");
    assert_ne!(second.version, first.version);
    assert_eq!(names_of(&service).await, ["dev-shop"]);
}

#[tokio::test]
async fn detaching_falls_back_to_a_dev_certificate() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    let uploaded = service
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
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(uploaded)))
        .await
        .unwrap();
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    let handle = certificate_for(&service, "shop.example.com").unwrap();
    assert_eq!(handle.name.as_str(), "dev-shop");
    service
        .delete_certificate(&ChangeContext::system(), uploaded)
        .await
        .unwrap();
    assert_eq!(names_of(&service).await, ["dev-shop"]);
}

#[tokio::test]
async fn startup_fills_a_certificate_gap_while_tls_is_on() {
    let env = Env::new();
    {
        let service = env.start(env.settings(false)).await;
        let site = add_shop(&service).await;
        service
            .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
            .await
            .unwrap();
    }
    assert!(!env.key_file().exists());
    let service = env.start(env.settings(true)).await;
    let handle = certificate_for(&service, "shop.example.com").expect("generated at startup");
    assert_eq!(handle.name.as_str(), "dev-shop");
    assert!(service.snapshot_holder().load().tls_enabled);
    let history = service
        .read(|conn| fast_proxy_store::revision::list(conn, 1, None))
        .await
        .unwrap();
    assert!(history.value[0].summary.contains("startup"));
}

#[tokio::test]
async fn a_restart_with_tls_on_and_nothing_missing_writes_nothing() {
    let env = Env::new();
    let version = {
        let service = env.start(env.settings(true)).await;
        let site = add_shop(&service).await;
        service
            .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
            .await
            .unwrap();
        service.snapshot_holder().load().version
    };
    let restarted = env.start(env.settings(true)).await;
    assert_eq!(restarted.snapshot_holder().load().version, version);
    assert!(certificate_for(&restarted, "shop.example.com").is_some());
}

#[tokio::test]
async fn an_invalid_upload_changes_nothing() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let ca = TestCa::new("ca");
    let good = leaf(&ca, &["a.example.com"]);
    let other = leaf(&ca, &["b.example.com"]);
    let expired = ca.issue(&LeafSpec::new(&["a.example.com"]).expiring_in_days(-3));
    let version = service.snapshot_holder().load().version;
    let cases = [
        ("x", "garbage", good.key_pem.as_str(), "certPem"),
        ("x", good.cert_pem.as_str(), "garbage", "keyPem"),
        (
            "x",
            good.cert_pem.as_str(),
            other.key_pem.as_str(),
            "keyPem",
        ),
        (
            "x",
            expired.cert_pem.as_str(),
            expired.key_pem.as_str(),
            "certPem",
        ),
        (
            "bad name!",
            good.cert_pem.as_str(),
            good.key_pem.as_str(),
            "name",
        ),
    ];
    for (name, cert, key, field) in cases {
        let err = service
            .upload_certificate(&ChangeContext::system(), name, cert, key)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, StoreError::Invalid(f) if f[0].field == field),
            "{field}: {err}"
        );
    }
    assert_eq!(service.snapshot_holder().load().version, version);
    assert!(service.list_certificates().await.unwrap().is_empty());
    assert!(
        !env.key_file().exists(),
        "a rejected upload creates no master key"
    );
}

#[tokio::test]
async fn a_duplicate_name_is_a_conflict_and_a_used_certificate_cannot_be_deleted() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    let id = service
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
    let dup = service
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &issued.chain_pem,
            &issued.key_pem,
        )
        .await
        .unwrap_err();
    assert!(matches!(dup, StoreError::Conflict(_)));
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    let err = service
        .delete_certificate(&ChangeContext::system(), id)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("shop")),
        "{err}"
    );
    assert_eq!(names_of(&service).await, ["shop"]);
}

#[tokio::test]
async fn attaching_a_certificate_needs_it_to_cover_a_host_and_reports_the_rest() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            move |conn| {
                hosts::add(conn, site, &HostPattern::parse("old.example.com").unwrap())?;
                Ok(Mutation {
                    value: (),
                    summary: "host added".into(),
                })
            },
        )
        .await
        .unwrap();
    let ca = TestCa::new("ca");
    let wrong = leaf(&ca, &["unrelated.example.com"]);
    let wrong_id = service
        .upload_certificate(
            &ChangeContext::system(),
            "wrong",
            &wrong.chain_pem,
            &wrong.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    let before = certificate_for(&service, "shop.example.com");
    let err = service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(wrong_id)))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].field == "https.certificateId"),
        "{err}"
    );
    assert!(before.is_none() && certificate_for(&service, "shop.example.com").is_none());

    let partial = leaf(&ca, &["shop.example.com"]);
    let partial_id = service
        .upload_certificate(
            &ChangeContext::system(),
            "partial",
            &partial.chain_pem,
            &partial.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    let applied = service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(partial_id)))
        .await
        .unwrap();
    assert_eq!(applied.value, ["old.example.com"]);

    let missing = service
        .set_subsystem_https(
            &ChangeContext::system(),
            site,
            https_on(Some(CertificateId(99))),
        )
        .await
        .unwrap_err();
    assert!(matches!(&missing, StoreError::Invalid(f) if f[0].field == "https.certificateId"));
}

#[tokio::test]
async fn renewing_with_the_same_key_replaces_one_entry_and_keeps_the_rest() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    // One key, two certificates: issue the renewal for the same key pair.
    let first = leaf(&ca, &["shop.example.com"]);
    let id = service
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &first.chain_pem,
            &first.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    let before = certificate_for(&service, "shop.example.com").unwrap();

    let other = leaf(&ca, &["shop.example.com"]);
    let err = service
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                cert_pem: Some(other.chain_pem.clone()),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].field == "keyPem"),
        "a different key does not match"
    );

    service
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                cert_pem: Some(other.chain_pem),
                key_pem: Some(other.key_pem),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    let after = certificate_for(&service, "shop.example.com").unwrap();
    assert!(!Arc::ptr_eq(&before, &after));
    assert_ne!(before.metadata.fingerprint, after.metadata.fingerprint);
}

#[tokio::test]
async fn renaming_reseals_the_key_and_a_replacement_must_still_cover_a_host() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    let id = service
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
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    service
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                name: Some("renamed".into()),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(names_of(&service).await, ["renamed"]);
    assert_eq!(
        certificate_for(&service, "shop.example.com")
            .unwrap()
            .name
            .as_str(),
        "renamed"
    );
    drop(service);
    let restarted = env.start(env.settings(true)).await;
    assert!(
        certificate_for(&restarted, "shop.example.com").is_some(),
        "the re-sealed key opens after a restart"
    );

    let elsewhere = leaf(&ca, &["unrelated.example.com"]);
    let err = restarted
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                cert_pem: Some(elsewhere.chain_pem),
                key_pem: Some(elsewhere.key_pem),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].field == "certPem"),
        "{err}"
    );
}

#[tokio::test]
async fn a_vetoed_write_leaves_the_certificates_and_the_cache_as_they_were() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    let before = certificate_for(&service, "shop.example.com").unwrap();
    let version = service.snapshot_holder().load().version;

    // A subsystem with no host has nothing to certify, so HTTPS cannot be enabled.
    let hostless = service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            |conn| {
                let id = subsystems::create(
                    conn,
                    &SubsystemFields {
                        name: fast_proxy_model::SubsystemName::parse("hostless").unwrap(),
                        enabled: true,
                        description: String::new(),
                        https: HttpsPolicy::default(),
                    },
                )?;
                Ok(Mutation {
                    value: id,
                    summary: "subsystem created".into(),
                })
            },
        )
        .await
        .unwrap()
        .value;
    let version = version + 1;
    let err = service
        .set_subsystem_https(&ChangeContext::system(), hostless, https_on(None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no certificate"), "{err}");

    assert_eq!(service.snapshot_holder().load().version, version);
    let after = certificate_for(&service, "shop.example.com").unwrap();
    assert!(Arc::ptr_eq(&before, &after), "a veto keeps the same handle");
    assert_eq!(service.list_certificates().await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_certificate_and_configuration_writes_serialize() {
    let env = Env::new();
    let service = Arc::new(env.start(env.settings(true)).await);
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    let context = ChangeContext::system();
    let (a, b, c) = tokio::join!(
        service.upload_certificate(&context, "one", &issued.chain_pem, &issued.key_pem),
        service.generate_self_signed(&context, "two", vec!["two.example.com".into()]),
        service.set_subsystem_https(&context, site, https_on(None)),
    );
    let mut versions = vec![a.unwrap().version, b.unwrap().version, c.unwrap().version];
    versions.sort_unstable();
    versions.dedup();
    assert_eq!(versions.len(), 3, "each write got its own revision");
    assert!(certificate_for(&service, "shop.example.com").is_some());
}

#[tokio::test]
async fn a_wrong_master_key_starts_reports_and_keeps_serving_plain_config() {
    let env = Env::new();
    {
        let service = env.start(env.settings(true)).await;
        let site = add_shop(&service).await;
        service
            .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
            .await
            .unwrap();
    }
    std::fs::write(
        env.key_file(),
        "AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=",
    )
    .unwrap();
    let service = env.start(env.settings(true)).await;
    assert_eq!(service.vault_status(), VaultStatus::KeyMismatch);
    let listed = service.list_certificates().await.unwrap();
    assert_eq!(listed[0].state.as_str(), "unreadable");
    assert!(
        certificate_for(&service, "shop.example.com").is_none(),
        "no handle for an unreadable key"
    );
    let err = service
        .generate_self_signed(
            &ChangeContext::system(),
            "more",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("master key")),
        "{err}"
    );
    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            create_site("blog", "blog.example.com".into(), "127.0.0.1:3001"),
        )
        .await
        .expect("configuration writes still work");
}

#[tokio::test]
async fn a_corrupt_certificate_row_is_skipped_and_reported_not_fatal() {
    let env = Env::new();
    {
        let service = env.start(env.settings(true)).await;
        let site = add_shop(&service).await;
        service
            .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
            .await
            .unwrap();
        let ca = TestCa::new("ca");
        let issued = leaf(&ca, &["other.example.com"]);
        service
            .upload_certificate(
                &ChangeContext::system(),
                "other",
                &issued.chain_pem,
                &issued.key_pem,
            )
            .await
            .unwrap();
    }
    {
        let store = Store::open(&env.database()).unwrap();
        store
            .run(|db| {
                db.read(|conn| {
                    conn.execute(
                        "UPDATE certificate SET chain_pem = 'garbage' WHERE name = 'dev-shop'",
                        [],
                    )?;
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
    let service = env.start(env.settings(true)).await;
    let states: Vec<_> = service
        .list_certificates()
        .await
        .unwrap()
        .into_iter()
        .map(|c| (c.name.as_str().to_string(), c.state.as_str()))
        .collect();
    assert_eq!(
        states,
        [("dev-shop".into(), "unreadable"), ("other".into(), "ok")]
    );
}

/// All bytes of the database file and its write-ahead log.
fn database_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = std::fs::read(path).unwrap();
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    bytes.extend(std::fs::read(PathBuf::from(wal)).unwrap_or_default());
    bytes
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[tokio::test]
async fn the_database_file_never_contains_private_key_text_or_bytes() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    let key_body: String = issued
        .key_pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let key_der = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(&key_body)
            .unwrap()
    };
    let id = service
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
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(Some(id)))
        .await
        .unwrap();
    service
        .replace_certificate(
            &ChangeContext::system(),
            id,
            CertificateChange {
                name: Some("renamed".into()),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    service
        .set_subsystem_https(&ChangeContext::system(), site, https_on(None))
        .await
        .unwrap();
    service
        .delete_certificate(&ChangeContext::system(), id)
        .await
        .unwrap();

    let bytes = database_bytes(&env.database());
    assert!(
        !contains(&bytes, b"PRIVATE KEY"),
        "no PEM header in the database"
    );
    assert!(!contains(&bytes, key_body.as_bytes()), "no base64 key body");
    assert!(
        !contains(&bytes, &key_der[key_der.len() - 40..]),
        "no raw key bytes"
    );
    assert!(
        contains(&bytes, b"BEGIN CERTIFICATE"),
        "the public chain is stored"
    );
}

#[tokio::test]
async fn certificate_changes_are_summary_only_revisions_without_pem() {
    let env = Env::new();
    let service = env.start(env.settings(true)).await;
    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["shop.example.com"]);
    service
        .upload_certificate(
            &ChangeContext::system(),
            "shop",
            &issued.chain_pem,
            &issued.key_pem,
        )
        .await
        .unwrap();
    let rows = service
        .read(|conn| {
            let mut statement = conn.prepare("SELECT summary || config FROM revision")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .unwrap()
        .value;
    assert!(rows
        .iter()
        .any(|row| row.contains("certificate uploaded: shop")));
    assert!(rows
        .iter()
        .all(|row| !row.contains("BEGIN") && !row.contains("PRIVATE")));
}

#[tokio::test]
async fn expiry_and_cache_metrics_are_published_with_certificate_names() {
    let env = Env::new();
    let metrics = Arc::new(Metrics::new());
    let store = Store::open(&env.database()).unwrap();
    let service = ConfigService::start_with_tls(store, metrics.clone(), env.settings(true))
        .await
        .unwrap();
    service
        .generate_self_signed(
            &ChangeContext::system(),
            "scratch",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap();
    let text = metrics.render();
    assert!(
        text.contains("fastproxy_tls_cert_expiry_seconds{cert=\"scratch\",source=\"self_signed\"}"),
        "{text}"
    );
    assert!(text.contains("fastproxy_cert_cache_total{result=\"miss\"} 1"));
}

#[tokio::test]
async fn the_key_can_come_from_the_environment_without_any_file() {
    let env = Env::new();
    let settings = TlsSettings {
        enabled: true,
        master_key_file: None,
        env_master_key: Some("AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=".into()),
    };
    let service = env.start(settings.clone()).await;
    service
        .generate_self_signed(
            &ChangeContext::system(),
            "scratch",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap();
    assert!(!env.key_file().exists());
    drop(service);
    let restarted = env.start(settings).await;
    assert_eq!(restarted.vault_status(), VaultStatus::Ok);
    assert_eq!(
        restarted.list_certificates().await.unwrap()[0]
            .state
            .as_str(),
        "ok"
    );
}

#[tokio::test]
async fn without_any_master_key_source_sealing_is_refused_clearly() {
    let env = Env::new();
    let service = env.start(TlsSettings::default()).await;
    let err = service
        .generate_self_signed(
            &ChangeContext::system(),
            "scratch",
            vec!["x.example.com".into()],
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Conflict(m) if m.contains("no master key")),
        "{err}"
    );
}

#[tokio::test]
async fn a_certificate_is_generated_once_by_name_and_never_replaced() {
    let env = Env::new();
    let service = env.start(env.settings(false)).await;
    let context = ChangeContext::system();
    assert!(service.certificate_by_name("admin").is_none());

    let generated = service
        .ensure_self_signed_certificate(&context, "admin", vec!["localhost".into()])
        .await
        .unwrap();
    assert!(generated);
    let first = service.certificate_by_name("admin").expect("usable");
    assert_eq!(first.name.as_str(), "admin");

    let again = service
        .ensure_self_signed_certificate(&context, "admin", vec!["other.example.com".into()])
        .await
        .unwrap();
    assert!(!again, "an existing certificate is left alone");
    let second = service.certificate_by_name("admin").unwrap();
    assert_eq!(first.version, second.version);
    assert_eq!(second.metadata.names, ["localhost"]);
}

#[tokio::test]
async fn a_replaced_certificate_is_what_the_next_lookup_returns() {
    let env = Env::new();
    let service = env.start(env.settings(false)).await;
    let context = ChangeContext::system();
    service
        .ensure_self_signed_certificate(&context, "admin", vec!["localhost".into()])
        .await
        .unwrap();
    let before = service.certificate_by_name("admin").unwrap();

    let ca = TestCa::new("ca");
    let issued = leaf(&ca, &["localhost"]);
    service
        .replace_certificate(
            &context,
            before.id,
            CertificateChange {
                cert_pem: Some(issued.chain_pem.clone()),
                key_pem: Some(issued.key_pem.clone()),
                ..CertificateChange::default()
            },
        )
        .await
        .unwrap();
    let after = service.certificate_by_name("admin").unwrap();
    assert_eq!(after.id, before.id);
    assert_ne!(after.version.fingerprint, before.version.fingerprint);
}

#[tokio::test]
async fn a_name_that_is_not_stored_or_not_readable_finds_nothing() {
    let env = Env::new();
    let service = env.start(env.settings(false)).await;
    assert!(service.certificate_by_name("nope").is_none());
}
