//! The ACME runner (RN1, RN2): one per instance. It works only while it holds
//! the renewal lease; then it looks at every subsystem with ACME on, asks the
//! pure [`RenewalPolicy`] what to do, runs the orders one at a time and records
//! the status. It looks again when the configuration changes, when an admin
//! asks for a renewal, and otherwise when the next thing is due.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_acme::{
    eligible_names, AcmeClient, AcmeError, AcmeSession, CertificateFacts, Decision, OrderEvent,
    OrderStep, RenewalFacts,
};
use fast_proxy_config_service::{CertificateSummary, ConfigService};
use fast_proxy_model::{AcmeState, AcmeStatus, CertificateSource, SubsystemConfig, SubsystemId};
use fast_proxy_store::repository::load;
use fast_proxy_telemetry::{AcmeOrderOutcome, Metrics};
use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use crate::order::{drive, OrderContext};
use crate::RunnerSettings;

/// The shortest wait between two passes, so a burst of changes is one pass.
const MIN_WAIT: Duration = Duration::from_secs(1);
/// Forced renewals waiting to be served, at most.
const REQUEST_QUEUE: usize = 16;

/// Asks the runner for work from outside (the admin API).
#[derive(Debug, Clone)]
pub struct AcmeControl {
    requests: mpsc::Sender<SubsystemId>,
}

impl AcmeControl {
    /// Asks for `subsystem` to be ordered now, skipping the backoff but never
    /// the hourly cap. Returns `false` when the queue is full.
    pub fn request_renewal(&self, subsystem: SubsystemId) -> bool {
        self.requests.try_send(subsystem).is_ok()
    }
}

/// The runner. Build it with [`AcmeRunner::new`] and spawn [`AcmeRunner::run`].
pub struct AcmeRunner {
    service: Arc<ConfigService>,
    metrics: Arc<Metrics>,
    settings: RunnerSettings,
    client: AcmeClient,
    session: Option<AcmeSession>,
    /// The eligible names last seen per subsystem, to notice a change.
    seen_names: HashMap<SubsystemId, Vec<String>>,
    /// When a subsystem's eligible names last changed (for the debounce).
    names_changed_at: HashMap<SubsystemId, i64>,
    requests: mpsc::Receiver<SubsystemId>,
}

impl AcmeRunner {
    pub fn new(
        service: Arc<ConfigService>,
        metrics: Arc<Metrics>,
        settings: RunnerSettings,
    ) -> (Self, AcmeControl) {
        let (sender, requests) = mpsc::channel(REQUEST_QUEUE);
        let client = AcmeClient::new(settings.directory.clone(), settings.directory_ca.clone());
        let runner = Self {
            service,
            metrics,
            settings,
            client,
            session: None,
            seen_names: HashMap::new(),
            names_changed_at: HashMap::new(),
            requests,
        };
        (runner, AcmeControl { requests: sender })
    }

    /// Runs until `shutdown`: takes or renews the lease every third of its
    /// lifetime and, while holding it, does a pass whenever something may be
    /// due. Releases the lease on the way out.
    pub async fn run(mut self, shutdown: CancellationToken) {
        let mut changes = self.service.subscribe();
        let renew_lease_every = self.settings.lease_ttl / 3;
        loop {
            let held = self.hold_lease().await;
            let mut wait = renew_lease_every;
            if held {
                wait = wait.min(self.run_once(None).await);
            }
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
                changed = changes.recv() => {
                    if matches!(changed, Err(broadcast::error::RecvError::Closed)) {
                        break;
                    }
                }
                Some(subsystem) = self.requests.recv() => {
                    if held {
                        self.run_once(Some(subsystem)).await;
                    }
                }
            }
        }
        if let Err(err) = self.service.release_acme_lease().await {
            tracing::warn!("acme: could not release the renewal lease: {err}");
        }
        self.metrics.set_acme_lease_held(false);
    }

    async fn hold_lease(&self) -> bool {
        let held = match self.service.try_acme_lease(self.settings.lease_ttl).await {
            Ok(held) => held,
            Err(err) => {
                tracing::warn!("acme: could not take the renewal lease: {err}");
                false
            }
        };
        self.metrics.set_acme_lease_held(held);
        held
    }

    /// One pass over every subsystem with ACME on, ordering what is due (and
    /// `forced`, if given). Returns how long until something may be due.
    /// The caller must hold the lease.
    pub async fn run_once(&mut self, forced: Option<SubsystemId>) -> Duration {
        let check_interval = self.settings.check_interval;
        let (subsystems, certificates, statuses) = match self.load().await {
            Ok(loaded) => loaded,
            Err(err) => {
                tracing::warn!("acme: could not read the configuration: {err}");
                return check_interval;
            }
        };
        let now = (self.settings.clock)();
        let mut wake = now.saturating_add(millis(check_interval));
        self.forget_inactive(&subsystems, &statuses).await;
        for subsystem in &subsystems {
            let status = statuses
                .get(&subsystem.id)
                .cloned()
                .unwrap_or_else(|| AcmeStatus::new(subsystem.id));
            let next = self
                .handle(
                    subsystem,
                    &certificates,
                    status,
                    forced == Some(subsystem.id),
                )
                .await;
            wake = wake.min(next);
        }
        let now = (self.settings.clock)();
        Duration::from_millis(u64::try_from(wake.saturating_sub(now)).unwrap_or(0))
            .clamp(MIN_WAIT, check_interval)
    }

    /// The subsystems with ACME on, the stored certificates and the statuses.
    async fn load(
        &self,
    ) -> Result<
        (
            Vec<SubsystemConfig>,
            Vec<CertificateSummary>,
            HashMap<SubsystemId, AcmeStatus>,
        ),
        fast_proxy_store::StoreError,
    > {
        let config = self.service.read(load::load_all).await?.value;
        let subsystems = config
            .subsystems
            .into_iter()
            .filter(|s| s.https.enabled && s.https.acme_enabled)
            .collect();
        let certificates = self.service.list_certificates().await?;
        let statuses = self
            .service
            .acme_statuses()
            .await?
            .into_iter()
            .map(|status| (status.subsystem_id, status))
            .collect();
        Ok((subsystems, certificates, statuses))
    }

    /// Drops the status of subsystems that no longer use ACME.
    async fn forget_inactive(
        &mut self,
        active: &[SubsystemConfig],
        statuses: &HashMap<SubsystemId, AcmeStatus>,
    ) {
        for id in statuses.keys() {
            if active.iter().any(|s| s.id == *id) {
                continue;
            }
            self.seen_names.remove(id);
            self.names_changed_at.remove(id);
            if let Err(err) = self.service.forget_acme_status(*id).await {
                tracing::warn!("acme: could not drop a status: {err}");
            }
        }
    }

    /// Decides for one subsystem and acts. Returns when to look again.
    async fn handle(
        &mut self,
        subsystem: &SubsystemConfig,
        certificates: &[CertificateSummary],
        mut status: AcmeStatus,
        forced: bool,
    ) -> i64 {
        let now = (self.settings.clock)();
        let eligibility = eligible_names(&subsystem.hosts, &self.settings.allowlist);
        self.note_names(subsystem.id, &eligibility.eligible, now);
        let certificate = subsystem.https.certificate_id.and_then(|id| {
            certificates
                .iter()
                .find(|c| c.id == id && c.source == CertificateSource::Acme)
        });
        let before = status.clone();
        status.eligible_names = eligibility.eligible.clone();
        status.skipped_names = eligibility.skipped;
        status.certificate_id = certificate.map(|c| c.id);
        let decision = self.settings.policy.decide(&RenewalFacts {
            now_ms: now,
            subsystem: subsystem.id,
            eligible: &eligibility.eligible,
            certificate: certificate.map(|c| CertificateFacts {
                names: &c.metadata.names,
                not_before_ms: c.metadata.not_before_ms,
                not_after_ms: c.metadata.not_after_ms,
            }),
            status: &status,
            forced,
            names_changed_at_ms: self.names_changed_at.get(&subsystem.id).copied(),
        });
        match decision {
            Decision::Order(_) => {
                self.order(
                    subsystem,
                    eligibility.eligible,
                    status,
                    certificate.is_some(),
                )
                .await;
                return now;
            }
            Decision::Idle => status.state = AcmeState::Idle,
            // No order runs between passes, so `pending` or `renewing` here was
            // left by an instance that stopped mid-order.
            Decision::NotDueUntil(_)
                if matches!(
                    status.state,
                    AcmeState::Idle | AcmeState::Pending | AcmeState::Renewing
                ) =>
            {
                status.state = AcmeState::Valid;
            }
            Decision::CapReached { until_ms } => {
                tracing::info!(
                    subsystem = subsystem.name.as_str(),
                    "acme: a forced renewal was refused: the hourly failure cap is spent until {until_ms}"
                );
            }
            Decision::WaitUntil(_) | Decision::NotDueUntil(_) => {}
        }
        if status != before {
            self.record(status).await;
        }
        match decision {
            Decision::WaitUntil(at) | Decision::NotDueUntil(at) => at,
            Decision::CapReached { until_ms } => until_ms,
            Decision::Idle | Decision::Order(_) => i64::MAX,
        }
    }

    /// Remembers the eligible names; a change after the first look starts the
    /// debounce (a first look is not a change: there is nothing to batch).
    fn note_names(&mut self, subsystem: SubsystemId, eligible: &[String], now: i64) {
        match self.seen_names.insert(subsystem, eligible.to_vec()) {
            Some(previous) if previous != eligible => {
                self.names_changed_at.insert(subsystem, now);
            }
            _ => {}
        }
    }

    /// Runs one order and records how it ended.
    async fn order(
        &mut self,
        subsystem: &SubsystemConfig,
        names: Vec<String>,
        mut status: AcmeStatus,
        has_certificate: bool,
    ) {
        let policy = self.settings.policy;
        policy.record_started(&mut status, has_certificate, (self.settings.clock)());
        self.record(status.clone()).await;
        let deadline =
            self.settings.ca_timeout * 2 + self.settings.propagation_wait + Duration::from_secs(60);
        let mut step = OrderStep::Starting;
        let context = OrderContext {
            service: &self.service,
            client: &self.client,
            settings: &self.settings,
            session: &mut self.session,
        };
        let outcome =
            tokio::time::timeout(deadline, drive(context, subsystem.id, &names, &mut step))
                .await
                .unwrap_or_else(|_| Err(AcmeError::transient("the order timed out")));
        let now = (self.settings.clock)();
        let name = subsystem.name.as_str();
        match outcome {
            Ok(stored) => {
                policy.record_success(&mut status, stored.id, now);
                self.record(status).await;
                let outcome = if stored.renewed {
                    AcmeOrderOutcome::Renewed
                } else {
                    AcmeOrderOutcome::Issued
                };
                self.metrics.acme_order(name, outcome);
                tracing::info!(subsystem = name, "acme: certificate stored for {names:?}");
            }
            Err(err) => {
                if step == OrderStep::Starting {
                    // The account may be what failed: open it afresh next time.
                    self.session = None;
                }
                let failed = step.next(OrderEvent::Failed(err.kind)).unwrap_or(step);
                if failed.must_clear_challenges() {
                    if let Err(clear) = self.service.clear_acme_challenges(subsystem.id).await {
                        tracing::warn!("acme: could not clear the challenges: {clear}");
                    }
                }
                policy.record_failure(&mut status, err.kind, &err.detail, now);
                if let Err(write) = self.service.record_acme_failure(status).await {
                    tracing::warn!("acme: could not record a failure: {write}");
                }
                self.metrics.acme_order(name, order_outcome(failed));
                tracing::warn!(subsystem = name, "acme: the order failed: {err}");
            }
        }
    }

    async fn record(&self, status: AcmeStatus) {
        if let Err(err) = self.service.record_acme_status(status).await {
            tracing::warn!("acme: could not record a status: {err}");
        }
    }
}

fn order_outcome(failed: OrderStep) -> AcmeOrderOutcome {
    match failed.outcome() {
        Some("rate_limited") => AcmeOrderOutcome::RateLimited,
        Some("self_check_failed") => AcmeOrderOutcome::SelfCheckFailed,
        _ => AcmeOrderOutcome::Failed,
    }
}

fn millis(duration: Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}
