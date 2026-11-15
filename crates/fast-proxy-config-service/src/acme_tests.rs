//! ACME writes against a real database file and master key: the switch rules,
//! challenges in the snapshot (here and on a second instance), storing an
//! issued certificate, and the lease, status and account written without a
//! revision.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{
    AcmeChallenge, AcmeKeyType, AcmeState, AcmeStatus, AcmeToken, CertificateSource, HttpsPolicy,
    SubsystemId,
};
use fast_proxy_store::repository::audit::{self, AuditOutcome, AuditQuery};
use fast_proxy_store::{Store, StoreError};
use fast_proxy_telemetry::Metrics;
use fast_proxy_test_pki::{LeafSpec, TestCa};
use tempfile::TempDir;

use crate::tests::create_site;
use crate::{
    AcmeAccountCredentials, Audited, ChangeContext, ConfigService, IssuedCertificate, TlsSettings,
};

const SHOP: &str = "shop.example.com";
const PATH: &str = "/.well-known/acme-challenge/tok-1";

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn settings(&self, acme_enabled: bool) -> TlsSettings {
        TlsSettings {
            enabled: true,
            acme_enabled,
            master_key_file: Some(self.dir.path().join("master.key")),
            env_master_key: None,
        }
    }

    fn database(&self) -> PathBuf {
        self.dir.path().join("fast-proxy.db")
    }

    async fn start(&self, acme_enabled: bool) -> ConfigService {
        ConfigService::start_with_tls(
            Store::open(&self.database()).unwrap(),
            Arc::new(Metrics::new()),
            self.settings(acme_enabled),
        )
        .await
        .unwrap()
    }
}

async fn add_shop(service: &ConfigService) -> SubsystemId {
    service
        .apply(
            &ChangeContext::system(),
            Audited::new("test.change", "test"),
            create_site("shop", SHOP.into(), "127.0.0.1:3000"),
        )
        .await
        .unwrap()
        .value
}

/// The shop with HTTPS on (a development certificate) and its ACME switch on.
async fn acme_shop(service: &ConfigService) -> SubsystemId {
    let site = add_shop(service).await;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https(true, None))
        .await
        .unwrap();
    site
}

fn https(acme_enabled: bool, certificate: Option<fast_proxy_model::CertificateId>) -> HttpsPolicy {
    HttpsPolicy {
        enabled: true,
        certificate_id: certificate,
        acme_enabled,
        ..HttpsPolicy::default()
    }
}

fn challenge(site: SubsystemId, expires_at_ms: i64) -> AcmeChallenge {
    AcmeChallenge::new(
        AcmeToken::parse("tok-1").unwrap(),
        site,
        vec![SHOP.into()],
        "tok-1.thumbprint".into(),
        expires_at_ms,
    )
    .unwrap()
}

fn answer(service: &ConfigService) -> Option<String> {
    service
        .snapshot_holder()
        .load()
        .acme_challenges
        .answer(SHOP, PATH)
        .map(|answer| answer.to_string())
}

async fn attached(service: &ConfigService, site: SubsystemId) -> HttpsPolicy {
    service
        .read(move |conn| fast_proxy_store::repository::subsystems::get(conn, site))
        .await
        .unwrap()
        .value
        .https
}

async fn audit_actions(service: &ConfigService) -> Vec<(String, AuditOutcome, Option<u64>)> {
    service
        .read(|conn| audit::list(conn, &AuditQuery::default(), None, 100))
        .await
        .unwrap()
        .value
        .into_iter()
        .map(|row| (row.entry.action, row.entry.outcome, row.entry.revision))
        .collect()
}

/// A chain for `names` signed for the key of a fresh order.
fn issued(site: SubsystemId, ca: &TestCa, names: &[&str]) -> IssuedCertificate {
    let names: Vec<String> = names.iter().map(ToString::to_string).collect();
    let order = fast_proxy_tls::generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
    let chain_pem = ca.sign_csr(&order.csr_der, &LeafSpec::new(&[])).unwrap();
    IssuedCertificate {
        subsystem: site,
        chain_pem,
        key: order.key,
        names,
    }
}

// ---- CS1: the switch ----

#[tokio::test]
async fn turning_acme_on_is_refused_while_the_process_has_it_off() {
    let env = Env::new();
    let service = env.start(false).await;
    let site = add_shop(&service).await;
    let err = service
        .set_subsystem_https(&ChangeContext::system(), site, https(true, None))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(fields) if fields[0].field == "https.acmeEnabled"),
        "{err:?}"
    );
    assert!(!attached(&service, site).await.acme_enabled);
}

#[tokio::test]
async fn a_stored_switch_stays_inert_and_compiling_after_the_process_turns_acme_off() {
    let env = Env::new();
    let site = acme_shop(&env.start(true).await).await;
    let service = env.start(false).await;
    assert!(attached(&service, site).await.acme_enabled);
    // An unrelated HTTPS edit keeps the switch on without the process switch.
    let mut policy = attached(&service, site).await;
    policy.hsts.enabled = true;
    service
        .set_subsystem_https(&ChangeContext::system(), site, policy)
        .await
        .unwrap();
}

#[tokio::test]
async fn while_acme_is_on_another_certificate_cannot_be_attached() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    let ca = TestCa::new("ca");
    let leaf = ca.issue(&LeafSpec::new(&[SHOP]));
    let uploaded = service
        .upload_certificate(
            &ChangeContext::system(),
            "manual",
            &leaf.chain_pem,
            &leaf.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    let err = service
        .set_subsystem_https(&ChangeContext::system(), site, https(true, Some(uploaded)))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Invalid(fields) if fields[0].field == "https.certificateId"),
        "{err:?}"
    );
    // Turning ACME off and attaching it in one edit is fine.
    service
        .set_subsystem_https(&ChangeContext::system(), site, https(false, Some(uploaded)))
        .await
        .unwrap();
}

// ---- CS2: challenges ----

#[tokio::test]
async fn published_challenges_are_answered_at_once_and_cleared_with_a_revision_each() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    let before = service.snapshot_holder().load().version;

    service
        .publish_acme_challenges(site, vec![challenge(site, i64::MAX)])
        .await
        .unwrap();
    assert_eq!(answer(&service).as_deref(), Some("tok-1.thumbprint"));
    assert_eq!(service.snapshot_holder().load().version, before + 1);

    assert!(service.clear_acme_challenges(site).await.unwrap().value);
    assert_eq!(answer(&service), None);
    assert_eq!(service.snapshot_holder().load().version, before + 2);
    assert!(
        !service.clear_acme_challenges(site).await.unwrap().value,
        "nothing left"
    );
    assert_eq!(
        service.snapshot_holder().load().version,
        before + 2,
        "nothing written"
    );

    let actions: Vec<String> = audit_actions(&service)
        .await
        .into_iter()
        .map(|a| a.0)
        .collect();
    assert!(actions.contains(&"acme.challenge.publish".to_string()));
    assert!(actions.contains(&"acme.challenge.clear".to_string()));
}

#[tokio::test]
async fn an_expired_challenge_is_not_answered_and_a_foreign_one_is_refused() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    service
        .publish_acme_challenges(site, vec![challenge(site, 1)])
        .await
        .unwrap();
    assert_eq!(answer(&service), None);
    let err = service
        .publish_acme_challenges(site, vec![challenge(SubsystemId(99), i64::MAX)])
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");
}

#[tokio::test]
async fn another_instance_answers_a_challenge_after_its_sync_poll() {
    let env = Env::new();
    let writer = env.start(true).await;
    let site = acme_shop(&writer).await;
    let reader = env.start(true).await;
    assert_eq!(answer(&reader), None);
    writer
        .publish_acme_challenges(site, vec![challenge(site, i64::MAX)])
        .await
        .unwrap();
    reader.sync_once().await;
    assert_eq!(answer(&reader).as_deref(), Some("tok-1.thumbprint"));
}

// ---- CS3: the issued certificate ----

#[tokio::test]
async fn an_issued_certificate_replaces_the_dev_one_and_clears_the_challenges_in_one_write() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    service
        .publish_acme_challenges(site, vec![challenge(site, i64::MAX)])
        .await
        .unwrap();
    let before = service.snapshot_holder().load().version;

    let stored = service
        .store_acme_certificate(issued(site, &TestCa::new("acme ca"), &[SHOP]))
        .await
        .unwrap();
    assert_eq!(stored.version, before + 1, "one write");
    assert!(!stored.value.renewed);
    assert_eq!(
        attached(&service, site).await.certificate_id,
        Some(stored.value.id)
    );
    assert_eq!(answer(&service), None, "the challenges went with it");

    let certificates = service.list_certificates().await.unwrap();
    assert_eq!(certificates.len(), 1, "the dev certificate was dropped");
    assert_eq!(certificates[0].name.as_str(), "acme-shop");
    assert_eq!(certificates[0].source, CertificateSource::Acme);
    let handle = service.certificate_by_name("acme-shop").expect("usable");
    assert_eq!(handle.source, CertificateSource::Acme);

    let actions = audit_actions(&service).await;
    assert!(actions.iter().any(
        |(action, outcome, revision)| action == "certificate.acme.issue"
            && *outcome == AuditOutcome::Ok
            && *revision == Some(stored.version)
    ));
}

#[tokio::test]
async fn a_renewal_replaces_the_certificate_in_place() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    let ca = TestCa::new("acme ca");
    let first = service
        .store_acme_certificate(issued(site, &ca, &[SHOP]))
        .await
        .unwrap()
        .value;
    let second = service
        .store_acme_certificate(issued(site, &ca, &[SHOP]))
        .await
        .unwrap()
        .value;
    assert!(second.renewed);
    assert_eq!(second.id, first.id, "same id, so it is a replacement");
    assert_eq!(service.list_certificates().await.unwrap().len(), 1);
    assert!(audit_actions(&service)
        .await
        .iter()
        .any(|(action, ..)| action == "certificate.acme.renew"));
}

#[tokio::test]
async fn an_uploaded_certificate_is_kept_when_acme_takes_over() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = add_shop(&service).await;
    let ca = TestCa::new("ca");
    let leaf = ca.issue(&LeafSpec::new(&[SHOP]));
    let uploaded = service
        .upload_certificate(
            &ChangeContext::system(),
            "manual",
            &leaf.chain_pem,
            &leaf.key_pem,
        )
        .await
        .unwrap()
        .value
        .id;
    service
        .set_subsystem_https(&ChangeContext::system(), site, https(true, Some(uploaded)))
        .await
        .unwrap();
    service
        .store_acme_certificate(issued(site, &ca, &[SHOP]))
        .await
        .unwrap();
    let mut names: Vec<String> = service
        .list_certificates()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name.as_str().to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["acme-shop", "manual"]);
}

#[tokio::test]
async fn nothing_is_stored_for_a_chain_that_misses_a_name_or_a_switch_turned_off() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    let ca = TestCa::new("acme ca");
    let mut short = issued(site, &ca, &[SHOP]);
    short.names.push("www.shop.example.com".into());
    let err = service.store_acme_certificate(short).await.unwrap_err();
    assert!(matches!(err, StoreError::Invalid(_)), "{err:?}");

    service
        .set_subsystem_https(&ChangeContext::system(), site, https(false, None))
        .await
        .unwrap();
    let err = service
        .store_acme_certificate(issued(site, &ca, &[SHOP]))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)), "{err:?}");
    assert!(service
        .list_certificates()
        .await
        .unwrap()
        .iter()
        .all(|c| c.source != CertificateSource::Acme));
}

// ---- CS4: lease and status ----

#[tokio::test]
async fn the_lease_is_held_by_one_instance_and_moves_when_released() {
    let env = Env::new();
    let first = env.start(true).await;
    let second = env.start(true).await;
    let version = first.snapshot_holder().load().version;
    let ttl = Duration::from_secs(60);
    assert!(first.try_acme_lease(ttl).await.unwrap());
    assert!(!second.try_acme_lease(ttl).await.unwrap());
    assert!(
        first.try_acme_lease(ttl).await.unwrap(),
        "the holder renews"
    );
    assert_eq!(
        first.acme_lease().await.unwrap().unwrap().holder,
        first.instance_id()
    );
    assert!(first.release_acme_lease().await.unwrap());
    assert!(second.try_acme_lease(ttl).await.unwrap());
    let stored = first
        .read(fast_proxy_store::revision::current_version)
        .await;
    assert_eq!(
        stored.unwrap().value,
        version,
        "no lease change is a revision"
    );
}

#[tokio::test]
async fn status_is_stored_without_a_revision_and_a_failure_is_audited() {
    let env = Env::new();
    let service = env.start(true).await;
    let site = acme_shop(&service).await;
    let version = service.snapshot_holder().load().version;

    let mut status = AcmeStatus::new(site);
    status.state = AcmeState::Pending;
    service.record_acme_status(status.clone()).await.unwrap();
    let stored = service.acme_status(site).await.unwrap().unwrap();
    assert_eq!(stored.state, AcmeState::Pending);
    assert_eq!(stored.instance, service.instance_id());

    status.state = AcmeState::Failed;
    status.last_error = "dns: no such host".into();
    service.record_acme_failure(status).await.unwrap();
    assert_eq!(
        service.acme_statuses().await.unwrap()[0].state,
        AcmeState::Failed
    );
    let failure = audit_actions(&service)
        .await
        .into_iter()
        .find(|(action, ..)| action == "certificate.acme.fail")
        .expect("audited");
    assert_eq!((failure.1, failure.2), (AuditOutcome::Failed, None));

    assert!(service.forget_acme_status(site).await.unwrap());
    assert_eq!(service.acme_status(site).await.unwrap(), None);
    let current = service
        .read(fast_proxy_store::revision::current_version)
        .await;
    assert_eq!(
        current.unwrap().value,
        version,
        "status is never a revision"
    );
}

// ---- CS5: the account ----

#[tokio::test]
async fn the_account_is_sealed_at_rest_and_opens_for_its_directory_only() {
    let env = Env::new();
    let service = env.start(true).await;
    let secret = b"account-private-key-material".to_vec();
    let directory = "https://ca.example/directory";
    assert!(service.acme_account(directory).await.unwrap().is_none());
    service
        .save_acme_account(
            directory,
            "ops@example.com",
            &AcmeAccountCredentials::new(secret.clone()),
        )
        .await
        .unwrap();

    let account = service.acme_account(directory).await.unwrap().unwrap();
    assert_eq!(account.credentials.expose(), secret.as_slice());
    assert_eq!(account.email, "ops@example.com");
    assert!(!format!("{account:?}").contains("account-private"));
    assert!(service
        .acme_account("https://other/dir")
        .await
        .unwrap()
        .is_none());

    let file = std::fs::read(env.database()).unwrap();
    let wal = std::fs::read(env.dir.path().join("fast-proxy.db-wal")).unwrap_or_default();
    for bytes in [file, wal] {
        assert!(!bytes.windows(secret.len()).any(|w| w == secret.as_slice()));
    }
    assert!(audit_actions(&service)
        .await
        .iter()
        .any(|(action, ..)| action == "acme.account.save"));
}
