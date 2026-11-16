//! Starting the ACME runner from `[data_plane.acme]` (M4). Off, nothing runs
//! and nothing contacts a CA.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_acme::RenewalPolicy;
use fast_proxy_acme_service::{system_clock, AcmeRunner, RunnerSettings, SelfCheck};
use fast_proxy_admin::AcmeAdmin;
use fast_proxy_config_service::ConfigService;
use fast_proxy_model::DomainAllowlist;
use fast_proxy_telemetry::Metrics;
use tokio_util::sync::CancellationToken;

use crate::config::{AppConfig, ConfigError, LETS_ENCRYPT_STAGING};

/// The longest one wait on the CA may take (validation, issuance).
const CA_TIMEOUT: Duration = Duration::from_secs(120);
/// How long shutdown waits for the runner to release the lease.
const RELEASE_GRACE: Duration = Duration::from_secs(5);

/// The runner's settings, or `None` when ACME is off for the process.
pub fn runner_settings(config: &AppConfig) -> Result<Option<RunnerSettings>, ConfigError> {
    let acme = &config.data_plane.acme;
    if !acme.enabled {
        return Ok(None);
    }
    let allowlist = DomainAllowlist::parse(&acme.allowed_domains)
        .map_err(|err| ConfigError::Invalid(format!("data_plane.acme.{err}")))?;
    Ok(Some(RunnerSettings {
        directory: acme.directory.clone(),
        directory_ca: acme.directory_ca.clone(),
        email: acme.email.clone(),
        allowlist,
        key_type: acme.key_type,
        check_interval: acme.renew_check_interval,
        propagation_wait: acme
            .propagation_wait
            .unwrap_or_else(|| default_propagation_wait(config.data_plane.config_poll_interval)),
        lease_ttl: acme.lease_ttl,
        self_check: if acme.self_check {
            SelfCheck::PublicName
        } else {
            SelfCheck::Off
        },
        ca_timeout: CA_TIMEOUT,
        policy: RenewalPolicy::default(),
        clock: system_clock(),
    }))
}

/// Twice the sync poll plus two seconds, so every instance has polled once
/// after the challenges were written; nothing when the poll is off.
fn default_propagation_wait(poll: Duration) -> Duration {
    if poll.is_zero() {
        return Duration::ZERO;
    }
    poll * 2 + Duration::from_secs(2)
}

/// A runner built but not started, so the admin plane can be given its
/// control before anything runs.
pub struct PreparedAcme(AcmeRunner);

/// Builds the runner, and what the admin plane needs to show ACME and to ask
/// the runner for a renewal.
pub fn prepare(
    settings: RunnerSettings,
    service: Arc<ConfigService>,
    metrics: Arc<Metrics>,
) -> (PreparedAcme, AcmeAdmin) {
    tracing::info!(
        "acme: on, directory {} (the account is created on the first order)",
        settings.directory
    );
    let directory = settings.directory.clone();
    let staging = directory == LETS_ENCRYPT_STAGING;
    let allowlist = settings.allowlist.clone();
    let (runner, control) = AcmeRunner::new(service, metrics, settings);
    let admin = AcmeAdmin {
        directory,
        staging,
        allowlist,
        control,
    };
    (PreparedAcme(runner), admin)
}

impl PreparedAcme {
    /// Starts the runner; it stops with `shutdown`.
    pub fn start(self, shutdown: CancellationToken) -> AcmeTask {
        AcmeTask(tokio::spawn(self.0.run(shutdown)))
    }
}

/// A running runner; [`AcmeTask::stopped`] waits for it to release the lease.
pub struct AcmeTask(tokio::task::JoinHandle<()>);

impl AcmeTask {
    /// Waits a little for the runner to give the lease up after shutdown.
    pub async fn stopped(self) {
        if tokio::time::timeout(RELEASE_GRACE, self.0).await.is_err() {
            tracing::warn!("acme: the runner did not stop in time; its lease will expire");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_builds_nothing() {
        assert!(runner_settings(&AppConfig::default()).unwrap().is_none());
    }

    #[test]
    fn on_carries_the_file_settings_and_derives_the_propagation_wait() {
        let mut config = AppConfig::default();
        config.data_plane.acme.enabled = true;
        config.data_plane.acme.allowed_domains = vec!["*.example.com".into()];
        config.data_plane.acme.self_check = false;
        config.data_plane.config_poll_interval = Duration::from_secs(5);
        let settings = runner_settings(&config).unwrap().unwrap();
        assert_eq!(settings.propagation_wait, Duration::from_secs(12));
        assert_eq!(settings.self_check, SelfCheck::Off);
        assert!(settings.allowlist.allows("shop.example.com"));
        assert_eq!(settings.check_interval, Duration::from_secs(3600));

        config.data_plane.config_poll_interval = Duration::ZERO;
        config.data_plane.acme.propagation_wait = None;
        config.data_plane.acme.self_check = true;
        let settings = runner_settings(&config).unwrap().unwrap();
        assert_eq!(settings.propagation_wait, Duration::ZERO);
        assert_eq!(settings.self_check, SelfCheck::PublicName);

        config.data_plane.acme.propagation_wait = Some(Duration::from_secs(3));
        assert_eq!(
            runner_settings(&config).unwrap().unwrap().propagation_wait,
            Duration::from_secs(3)
        );
    }

    #[test]
    fn a_bad_allowlist_is_a_config_error() {
        let mut config = AppConfig::default();
        config.data_plane.acme.enabled = true;
        assert!(runner_settings(&config).is_err());
    }
}
