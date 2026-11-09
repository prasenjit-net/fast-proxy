use std::time::Duration;

use crate::bounds::{check_millis, check_range};
use crate::FieldError;

const MIN_INTERVAL_MS: u64 = 100;
const MAX_INTERVAL_MS: u64 = 600_000;
const MAX_PATH_LEN: usize = 512;

/// Which active probe a group runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum HealthKind {
    /// No active probing (passive ejection still applies).
    #[default]
    None,
    /// An HTTP `GET` of the configured path.
    Http,
    /// A TCP connect.
    Tcp,
}

impl HealthKind {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Http => "http",
            Self::Tcp => "tcp",
        }
    }

    /// Parses the stored and API spelling.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "none" => Ok(Self::None),
            "http" => Ok(Self::Http),
            "tcp" => Ok(Self::Tcp),
            _ => Err(FieldError::new("health.check", "must be none, http or tcp")),
        }
    }
}

/// Active health-check settings of a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthCheck {
    pub kind: HealthKind,
    pub path: String,
    pub status_min: u16,
    pub status_max: u16,
    pub interval: Duration,
    pub timeout: Duration,
    /// Consecutive good probes that mark an endpoint healthy.
    pub rise: u32,
    /// Consecutive failed probes that mark an endpoint unhealthy.
    pub fall: u32,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            kind: HealthKind::None,
            path: "/".to_string(),
            status_min: 200,
            status_max: 399,
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(2),
            rise: 2,
            fall: 3,
        }
    }
}

impl HealthCheck {
    /// Checks every bound; errors name the field in the API's dotted spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        if !self.path.starts_with('/')
            || self.path.len() > MAX_PATH_LEN
            || self
                .path
                .chars()
                .any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(FieldError::new(
                "health.path",
                "must start with / and contain no spaces (at most 512 characters)",
            ));
        }
        let status_ok = |code: u16| (100..=599).contains(&code);
        if !status_ok(self.status_min)
            || !status_ok(self.status_max)
            || self.status_min > self.status_max
        {
            return Err(FieldError::new(
                "health.expectedStatus",
                "must be a range of HTTP statuses from 100 to 599, lowest first",
            ));
        }
        check_millis(
            "health.intervalMs",
            self.interval,
            MIN_INTERVAL_MS,
            MAX_INTERVAL_MS,
        )?;
        check_millis(
            "health.timeoutMs",
            self.timeout,
            MIN_INTERVAL_MS,
            MAX_INTERVAL_MS,
        )?;
        if self.timeout > self.interval {
            return Err(FieldError::new(
                "health.timeoutMs",
                "must not exceed the interval",
            ));
        }
        check_range("health.rise", self.rise, 1, 10)?;
        check_range("health.fall", self.fall, 1, 10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Case = (&'static str, Box<dyn FnOnce(&mut HealthCheck)>);

    fn with(change: impl FnOnce(&mut HealthCheck)) -> Result<(), FieldError> {
        let mut check = HealthCheck::default();
        change(&mut check);
        check.validate()
    }

    #[test]
    fn defaults_are_valid_and_match_the_documented_values() {
        let check = HealthCheck::default();
        assert!(check.validate().is_ok());
        assert_eq!(check.kind, HealthKind::None);
        assert_eq!(
            (check.interval, check.timeout),
            (Duration::from_secs(5), Duration::from_secs(2))
        );
        assert_eq!((check.rise, check.fall), (2, 3));
        assert_eq!((check.status_min, check.status_max), (200, 399));
    }

    #[test]
    fn kinds_round_trip() {
        for kind in [HealthKind::None, HealthKind::Http, HealthKind::Tcp] {
            assert_eq!(HealthKind::parse(kind.as_str()).unwrap(), kind);
        }
        assert_eq!(HealthKind::parse("icmp").unwrap_err().field, "health.check");
    }

    #[test]
    fn every_bound_is_enforced_with_its_field() {
        let cases: Vec<Case> = vec![
            ("health.path", Box::new(|c| c.path = "healthz".into())),
            ("health.path", Box::new(|c| c.path = "/a b".into())),
            (
                "health.path",
                Box::new(|c| c.path = format!("/{}", "a".repeat(512))),
            ),
            ("health.expectedStatus", Box::new(|c| c.status_min = 99)),
            ("health.expectedStatus", Box::new(|c| c.status_max = 600)),
            (
                "health.expectedStatus",
                Box::new(|c| {
                    c.status_min = 300;
                    c.status_max = 200;
                }),
            ),
            (
                "health.intervalMs",
                Box::new(|c| c.interval = Duration::from_millis(99)),
            ),
            (
                "health.intervalMs",
                Box::new(|c| c.interval = Duration::from_millis(600_001)),
            ),
            (
                "health.timeoutMs",
                Box::new(|c| c.timeout = Duration::from_millis(99)),
            ),
            (
                "health.timeoutMs",
                Box::new(|c| c.timeout = Duration::from_secs(6)),
            ),
            ("health.rise", Box::new(|c| c.rise = 0)),
            ("health.rise", Box::new(|c| c.rise = 11)),
            ("health.fall", Box::new(|c| c.fall = 0)),
            ("health.fall", Box::new(|c| c.fall = 11)),
        ];
        for (field, change) in cases {
            assert_eq!(with(change).unwrap_err().field, field);
        }
    }

    #[test]
    fn the_inclusive_bounds_are_accepted() {
        assert!(with(|c| {
            c.interval = Duration::from_millis(100);
            c.timeout = Duration::from_millis(100);
        })
        .is_ok());
        assert!(with(|c| {
            c.interval = Duration::from_millis(600_000);
            c.timeout = Duration::from_millis(600_000);
        })
        .is_ok());
        assert!(with(|c| {
            c.rise = 10;
            c.fall = 10;
            c.status_min = 100;
            c.status_max = 599;
        })
        .is_ok());
    }
}
