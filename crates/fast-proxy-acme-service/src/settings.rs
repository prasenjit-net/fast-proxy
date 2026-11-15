//! What the runner needs to know, decided at startup from `[data_plane.acme]`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_acme::RenewalPolicy;
use fast_proxy_model::{AcmeKeyType, DomainAllowlist};

/// Wall-clock milliseconds since the epoch. Injected so tests can move time.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The system clock.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| {
                i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
            })
    })
}

/// Where a challenge is fetched before the CA is asked to (K8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheck {
    /// Not at all.
    Off,
    /// Through the public name on port 80, as the CA will.
    PublicName,
    /// From this address with the name in `Host` (tests: a local listener).
    At(SocketAddr),
}

/// The runner's settings.
#[derive(Clone)]
pub struct RunnerSettings {
    pub directory: String,
    pub directory_ca: Option<PathBuf>,
    pub email: String,
    pub allowlist: DomainAllowlist,
    pub key_type: AcmeKeyType,
    /// The longest the runner sleeps between looks at the certificates.
    pub check_interval: Duration,
    /// How long other instances get to see published challenges.
    pub propagation_wait: Duration,
    pub lease_ttl: Duration,
    pub self_check: SelfCheck,
    /// The longest one wait on the CA (validation, issuance) may take.
    pub ca_timeout: Duration,
    pub policy: RenewalPolicy,
    pub clock: Clock,
}

impl std::fmt::Debug for RunnerSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunnerSettings")
            .field("directory", &self.directory)
            .field("allowlist", &self.allowlist.patterns())
            .field("check_interval", &self.check_interval)
            .field("propagation_wait", &self.propagation_wait)
            .field("lease_ttl", &self.lease_ttl)
            .field("self_check", &self.self_check)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_clock_reads_wall_time() {
        let now = system_clock()();
        assert!(now > 1_700_000_000_000, "{now}");
    }

    #[test]
    fn debug_names_the_directory_and_allowlist_but_not_the_clock() {
        let settings = RunnerSettings {
            directory: "https://ca.example/dir".into(),
            directory_ca: None,
            email: "ops@example.com".into(),
            allowlist: DomainAllowlist::parse(&["*.example.com".to_string()]).unwrap(),
            key_type: AcmeKeyType::EcdsaP256,
            check_interval: Duration::from_secs(60),
            propagation_wait: Duration::ZERO,
            lease_ttl: Duration::from_secs(60),
            self_check: SelfCheck::Off,
            ca_timeout: Duration::from_secs(5),
            policy: RenewalPolicy::default(),
            clock: system_clock(),
        };
        let shown = format!("{settings:?}");
        assert!(
            shown.contains("https://ca.example/dir") && shown.contains("*.example.com"),
            "{shown}"
        );
        assert!(!shown.contains("clock"), "{shown}");
    }
}
