//! ACME issuance end to end (M4 exit criteria 1, 2, 3 and 6): the real runner,
//! the real data plane answering the challenge on its plain listener, and an
//! in-process ACME CA that validates by fetching from it. A clock the tests
//! move stands in for the days between renewals.

mod common;

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::tls_stack::{Options, Stack};
use fast_proxy_acme::RenewalPolicy;
use fast_proxy_acme_service::{AcmeRunner, RunnerSettings, SelfCheck};
use fast_proxy_config_service::{Audited, ChangeContext};
use fast_proxy_model::{
    AcmeKeyType, AcmeState, AcmeStatus, CertificateSource, DomainAllowlist, HostPattern,
    HttpsPolicy, RedirectMode, SkipReason, SubsystemId,
};
use fast_proxy_store::repository::audit::{self, AuditQuery};
use fast_proxy_store::repository::hosts;
use fast_proxy_store::Mutation;
use fast_proxy_test_acme::TestAcme;
use fast_proxy_test_pki::request;
use tokio_util::sync::CancellationToken;

const SHOP: &str = "shop.example.com";
const DAY_MS: i64 = 86_400_000;

/// The system clock plus an offset the test moves.
#[derive(Clone, Default)]
struct Clock(Arc<AtomicI64>);

impl Clock {
    fn advance(&self, ms: i64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    fn as_clock(&self) -> fast_proxy_acme_service::Clock {
        let offset = self.0.clone();
        Arc::new(move || now_ms() + offset.load(Ordering::SeqCst))
    }
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

struct Rig {
    stack: Stack,
    ca: TestAcme,
    clock: Clock,
    site: SubsystemId,
}

impl Rig {
    /// The shop with HTTPS on, redirecting plain HTTP, and ACME on; the CA
    /// fetches challenges from the plain listener.
    async fn start() -> Self {
        let stack = Stack::start(Options {
            acme: true,
            ..Options::default()
        })
        .await;
        let ca = TestAcme::start().await;
        ca.set_challenge_targets(vec![stack.http]);
        let site = stack.add_site("shop", SHOP).await;
        let https = HttpsPolicy {
            enabled: true,
            redirect: RedirectMode::Permanent308,
            acme_enabled: true,
            ..HttpsPolicy::default()
        };
        stack
            .service
            .set_subsystem_https(&ChangeContext::system(), site, https)
            .await
            .unwrap();
        Self {
            stack,
            ca,
            clock: Clock::default(),
            site,
        }
    }

    fn settings(&self) -> RunnerSettings {
        RunnerSettings {
            directory: self.ca.directory.clone(),
            directory_ca: Some(self.ca.directory_ca.clone()),
            email: "ops@example.com".into(),
            allowlist: DomainAllowlist::parse(&["*.example.com".to_string()]).unwrap(),
            key_type: AcmeKeyType::EcdsaP256,
            check_interval: Duration::from_secs(3600),
            propagation_wait: Duration::ZERO,
            lease_ttl: Duration::from_secs(60),
            self_check: SelfCheck::At(self.stack.http),
            ca_timeout: Duration::from_secs(20),
            policy: RenewalPolicy::default(),
            clock: self.clock.as_clock(),
        }
    }

    fn runner_with(&self, settings: RunnerSettings) -> AcmeRunner {
        AcmeRunner::new(
            self.stack.service.clone(),
            self.stack.metrics.clone(),
            settings,
        )
        .0
    }

    fn runner(&self) -> AcmeRunner {
        self.runner_with(self.settings())
    }

    async fn status(&self) -> AcmeStatus {
        self.stack
            .service
            .acme_status(self.site)
            .await
            .unwrap()
            .unwrap()
    }

    /// The attached certificate's source, id and names.
    async fn attached(&self) -> (CertificateSource, i64, Vec<String>) {
        let site = self.site;
        let id = self
            .stack
            .service
            .read(move |conn| fast_proxy_store::repository::subsystems::get(conn, site))
            .await
            .unwrap()
            .value
            .https
            .certificate_id
            .unwrap();
        let summary = self
            .stack
            .service
            .list_certificates()
            .await
            .unwrap()
            .into_iter()
            .find(|c| c.id == id)
            .unwrap();
        (summary.source, id.0, summary.metadata.names)
    }

    /// A request over HTTPS from a client that trusts only the ACME issuer.
    async fn trusted_request(&self, host: &str) -> String {
        let hello = fast_proxy_test_pki::ClientHello {
            sni: Some(host.into()),
            trust_pem: vec![self.ca.issuer_pem.clone()],
            ..fast_proxy_test_pki::ClientHello::default()
        };
        let raw = format!("GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        request(self.stack.https, &hello, raw.as_bytes())
            .await
            .unwrap_or_else(|err| format!("handshake failed: {}", err.0))
    }

    async fn add_host(&self, host: &'static str) {
        let site = self.site;
        self.stack
            .service
            .apply(
                &ChangeContext::system(),
                Audited::new("host.add", host),
                move |conn| {
                    hosts::add(conn, site, &HostPattern::parse(host).unwrap())?;
                    Ok(Mutation {
                        value: (),
                        summary: format!("host added: {host}"),
                    })
                },
            )
            .await
            .unwrap();
    }

    async fn audit_actions(&self) -> Vec<String> {
        self.stack
            .service
            .read(|conn| audit::list(conn, &AuditQuery::default(), None, 200))
            .await
            .unwrap()
            .value
            .into_iter()
            .map(|row| row.entry.action)
            .collect()
    }
}

#[tokio::test]
async fn a_first_certificate_is_issued_validated_through_the_proxy_and_served_at_once() {
    let rig = Rig::start().await;
    assert!(
        rig.trusted_request(SHOP)
            .await
            .starts_with("handshake failed"),
        "dev certificate"
    );

    rig.runner().run_once(None).await;

    let status = rig.status().await;
    assert_eq!(status.state, AcmeState::Valid, "{status:?}");
    assert_eq!(status.eligible_names, [SHOP]);
    let (source, _, names) = rig.attached().await;
    assert_eq!(
        (source, names),
        (CertificateSource::Acme, vec![SHOP.to_string()])
    );
    let answer = rig.trusted_request(SHOP).await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");

    let validations = rig.ca.validations();
    assert_eq!(validations.len(), 1);
    assert!(
        validations[0].ok,
        "the CA fetched the key authorization through the redirect"
    );
    assert_eq!((rig.ca.orders(), rig.ca.accounts()), (1, 1));
    let actions = rig.audit_actions().await;
    for action in [
        "acme.account.save",
        "acme.challenge.publish",
        "certificate.acme.issue",
    ] {
        assert!(actions.iter().any(|a| a == action), "{action}: {actions:?}");
    }
    rig.stack
        .metric(r#"fastproxy_acme_orders_total{outcome="issued",subsystem="shop"} 1"#)
        .await;
}

#[tokio::test]
async fn a_status_left_renewing_by_a_stopped_instance_is_set_right_on_the_next_pass() {
    let rig = Rig::start().await;
    let mut runner = rig.runner();
    runner.run_once(None).await;
    let mut stale = rig.status().await;
    stale.state = AcmeState::Renewing;
    rig.stack.service.record_acme_status(stale).await.unwrap();
    runner.run_once(None).await;
    assert_eq!(rig.status().await.state, AcmeState::Valid);
    assert_eq!(rig.ca.orders(), 1);
}

#[tokio::test]
async fn a_valid_certificate_is_never_ordered_again_not_even_after_a_restart() {
    let rig = Rig::start().await;
    let mut runner = rig.runner();
    runner.run_once(None).await;
    let wait = runner.run_once(None).await;
    // The interval is counted from the pass's start, so the real time the pass
    // took (a millisecond on a busy machine) may come off it.
    assert!(
        (Duration::from_secs(3599)..=Duration::from_secs(3600)).contains(&wait),
        "nothing due before the check interval: {wait:?}"
    );
    // A new runner (a restart) opens the stored account and orders nothing.
    rig.runner().run_once(None).await;
    assert_eq!((rig.ca.orders(), rig.ca.accounts()), (1, 1));
}

#[tokio::test]
async fn a_certificate_is_renewed_in_place_once_two_thirds_of_its_lifetime_passed() {
    let rig = Rig::start().await;
    let mut runner = rig.runner();
    runner.run_once(None).await;
    let (_, first_id, _) = rig.attached().await;

    rig.clock.advance(50 * DAY_MS);
    runner.run_once(None).await;
    assert_eq!(rig.ca.orders(), 1, "not due at day 50");

    rig.clock.advance(11 * DAY_MS);
    runner.run_once(None).await;
    assert_eq!(rig.ca.orders(), 2, "due by day 61");
    let (source, id, _) = rig.attached().await;
    assert_eq!(
        (source, id),
        (CertificateSource::Acme, first_id),
        "replaced in place"
    );
    assert!(rig
        .audit_actions()
        .await
        .iter()
        .any(|a| a == "certificate.acme.renew"));
    assert_eq!(rig.status().await.state, AcmeState::Valid);
}

#[tokio::test]
async fn an_added_host_is_ordered_after_the_debounce_and_ineligible_ones_are_explained() {
    let rig = Rig::start().await;
    let mut runner = rig.runner();
    runner.run_once(None).await;

    rig.add_host("www.shop.example.com").await;
    rig.add_host("*.cdn.example.com").await;
    rig.add_host("shop.other.net").await;
    runner.run_once(None).await;
    assert_eq!(rig.ca.orders(), 1, "debounced");
    let skipped: Vec<(String, SkipReason)> = rig
        .status()
        .await
        .skipped_names
        .into_iter()
        .map(|s| (s.name, s.reason))
        .collect();
    assert_eq!(
        skipped,
        [
            (
                "*.cdn.example.com".to_string(),
                SkipReason::WildcardNeedsDns01
            ),
            ("shop.other.net".to_string(), SkipReason::NotAllowed),
        ]
    );

    rig.clock.advance(31_000);
    runner.run_once(None).await;
    assert_eq!(rig.ca.orders(), 2);
    let (_, _, names) = rig.attached().await;
    assert_eq!(names, [SHOP, "www.shop.example.com"]);
    let answer = rig.trusted_request("www.shop.example.com").await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

#[tokio::test]
async fn a_failed_validation_backs_off_clears_the_challenges_and_keeps_serving() {
    let rig = Rig::start().await;
    rig.ca.set_challenge_targets(vec![rig.stack.backend]);
    let mut runner = rig.runner();
    runner.run_once(None).await;

    let status = rig.status().await;
    assert_eq!(status.state, AcmeState::Failed, "{status:?}");
    assert!(
        status.last_error.starts_with("unauthorized:"),
        "{}",
        status.last_error
    );
    let wait_ms = status.next_attempt_at_ms.unwrap() - status.last_attempt_at_ms.unwrap();
    assert_eq!(wait_ms, 6 * 3_600_000, "a CA refusal waits long");
    assert!(rig
        .stack
        .service
        .snapshot_holder()
        .load()
        .acme_challenges
        .is_empty());
    let (source, _, _) = rig.attached().await;
    assert_eq!(
        source,
        CertificateSource::SelfSigned,
        "the dev certificate still serves"
    );
    assert!(rig
        .audit_actions()
        .await
        .iter()
        .any(|a| a == "certificate.acme.fail"));

    runner.run_once(None).await;
    assert_eq!(rig.ca.orders(), 1, "the backoff holds");
}

#[tokio::test]
async fn a_failed_self_check_spends_no_validation_at_the_ca() {
    let rig = Rig::start().await;
    let mut settings = rig.settings();
    settings.self_check = SelfCheck::At(rig.stack.backend);
    rig.runner_with(settings).run_once(None).await;
    let status = rig.status().await;
    assert_eq!(status.state, AcmeState::Failed);
    assert!(
        status.last_error.starts_with("self-check:"),
        "{}",
        status.last_error
    );
    assert!(rig.ca.validations().is_empty());
    rig.stack
        .metric(r#"fastproxy_acme_orders_total{outcome="self_check_failed",subsystem="shop"} 1"#)
        .await;
}

#[tokio::test]
async fn a_rate_limit_is_recorded_and_waited_out() {
    let rig = Rig::start().await;
    rig.ca.set_rate_limited(true);
    let mut runner = rig.runner();
    runner.run_once(None).await;
    let status = rig.status().await;
    assert_eq!(status.state, AcmeState::RateLimited);
    assert!(status.next_attempt_at_ms.unwrap() - status.last_attempt_at_ms.unwrap() >= 3_600_000);
    rig.ca.set_rate_limited(false);
    runner.run_once(None).await;
    assert_eq!(rig.ca.issued(), 0, "still waiting");
    rig.clock.advance(3_600_001);
    runner.run_once(None).await;
    assert_eq!(rig.ca.issued(), 1);
}

#[tokio::test]
async fn a_forced_renewal_skips_the_backoff_but_not_the_cap() {
    let rig = Rig::start().await;
    rig.ca.set_challenge_targets(vec![rig.stack.backend]);
    let mut runner = rig.runner();
    for _ in 0..5 {
        runner.run_once(Some(rig.site)).await;
    }
    assert_eq!(rig.ca.orders(), 5);
    runner.run_once(Some(rig.site)).await;
    assert_eq!(rig.ca.orders(), 5, "the hourly cap is spent");
    rig.ca.set_challenge_targets(vec![rig.stack.http]);
    rig.clock.advance(3_600_001);
    runner.run_once(Some(rig.site)).await;
    assert_eq!(rig.status().await.state, AcmeState::Valid);
}

#[tokio::test]
async fn the_running_runner_takes_the_lease_issues_and_releases_on_shutdown() {
    let rig = Rig::start().await;
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(rig.runner().run(shutdown.clone()));
    let mut issued = false;
    for _ in 0..200 {
        if rig.ca.issued() == 1 && rig.status().await.state == AcmeState::Valid {
            issued = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(issued, "the runner issued on its own");
    assert!(rig.stack.service.acme_lease().await.unwrap().is_some());
    rig.stack.metric("fastproxy_acme_lease_held 1").await;
    shutdown.cancel();
    task.await.unwrap();
    assert!(
        rig.stack.service.acme_lease().await.unwrap().is_none(),
        "released"
    );
    rig.stack.metric("fastproxy_acme_lease_held 0").await;
}

#[tokio::test]
async fn turning_acme_off_drops_the_status_and_keeps_the_certificate() {
    let rig = Rig::start().await;
    let mut runner = rig.runner();
    runner.run_once(None).await;
    let (_, id, _) = rig.attached().await;
    let mut https = HttpsPolicy {
        enabled: true,
        redirect: RedirectMode::Permanent308,
        ..HttpsPolicy::default()
    };
    https.certificate_id = Some(fast_proxy_model::CertificateId(id));
    rig.stack
        .service
        .set_subsystem_https(&ChangeContext::system(), rig.site, https)
        .await
        .unwrap();
    runner.run_once(None).await;
    assert!(rig
        .stack
        .service
        .acme_status(rig.site)
        .await
        .unwrap()
        .is_none());
    assert_eq!(rig.attached().await.0, CertificateSource::Acme);
}
