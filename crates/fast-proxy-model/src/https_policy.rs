use crate::bounds::check_range;
use crate::{CertificateId, FieldError};

/// Longest HSTS max-age accepted: two years.
pub const MAX_HSTS_AGE_SECS: u32 = 63_072_000;

/// What a plain-HTTP request to an HTTPS-enabled subsystem receives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RedirectMode {
    /// Served as usual.
    #[default]
    Off,
    /// `301 Moved Permanently`.
    Permanent301,
    /// `308 Permanent Redirect`, which keeps the method.
    Permanent308,
}

impl RedirectMode {
    /// The stored and API spelling: `off`, `301` or `308`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Permanent301 => "301",
            Self::Permanent308 => "308",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "off" => Ok(Self::Off),
            "301" => Ok(Self::Permanent301),
            "308" => Ok(Self::Permanent308),
            _ => Err(FieldError::new("https.redirect", "must be off, 301 or 308")),
        }
    }
}

/// HTTP Strict Transport Security sent on HTTPS responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hsts {
    pub enabled: bool,
    pub max_age_secs: u32,
    pub include_subdomains: bool,
}

impl Default for Hsts {
    fn default() -> Self {
        Self {
            enabled: false,
            max_age_secs: 31_536_000,
            include_subdomains: false,
        }
    }
}

/// How a subsystem is served over HTTPS (effective only while TLS is on for
/// the process). Redirect and HSTS settings need `enabled`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HttpsPolicy {
    pub enabled: bool,
    /// The attached certificate, if any.
    pub certificate_id: Option<CertificateId>,
    pub redirect: RedirectMode,
    pub hsts: Hsts,
}

impl HttpsPolicy {
    /// Checks bounds and that redirect and HSTS are only set with HTTPS enabled;
    /// errors name the field in the API's dotted spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range(
            "https.hsts.maxAgeSecs",
            self.hsts.max_age_secs,
            0,
            MAX_HSTS_AGE_SECS,
        )?;
        if self.enabled {
            return Ok(());
        }
        if self.redirect != RedirectMode::Off {
            return Err(FieldError::new("https.redirect", "requires https.enabled"));
        }
        if self.hsts.enabled {
            return Err(FieldError::new(
                "https.hsts.enabled",
                "requires https.enabled",
            ));
        }
        if self.hsts.include_subdomains {
            return Err(FieldError::new(
                "https.hsts.includeSubdomains",
                "requires https.enabled",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> HttpsPolicy {
        HttpsPolicy {
            enabled: true,
            ..HttpsPolicy::default()
        }
    }

    #[test]
    fn defaults_are_off_and_valid() {
        let policy = HttpsPolicy::default();
        assert!(!policy.enabled && policy.certificate_id.is_none());
        assert_eq!(policy.redirect, RedirectMode::Off);
        assert_eq!(policy.hsts.max_age_secs, 31_536_000);
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn redirect_modes_round_trip_and_reject_others() {
        for mode in [
            RedirectMode::Off,
            RedirectMode::Permanent301,
            RedirectMode::Permanent308,
        ] {
            assert_eq!(RedirectMode::parse(mode.as_str()).unwrap(), mode);
        }
        for bad in ["", "302", "OFF", "on"] {
            assert_eq!(
                RedirectMode::parse(bad).unwrap_err().field,
                "https.redirect"
            );
        }
    }

    #[test]
    fn redirect_and_hsts_need_https_enabled() {
        let cases: [(HttpsPolicy, &str); 3] = [
            (
                HttpsPolicy {
                    redirect: RedirectMode::Permanent308,
                    ..HttpsPolicy::default()
                },
                "https.redirect",
            ),
            (
                HttpsPolicy {
                    hsts: Hsts {
                        enabled: true,
                        ..Hsts::default()
                    },
                    ..HttpsPolicy::default()
                },
                "https.hsts.enabled",
            ),
            (
                HttpsPolicy {
                    hsts: Hsts {
                        include_subdomains: true,
                        ..Hsts::default()
                    },
                    ..HttpsPolicy::default()
                },
                "https.hsts.includeSubdomains",
            ),
        ];
        for (policy, field) in cases {
            assert_eq!(policy.validate().unwrap_err().field, field);
            let with_https = HttpsPolicy {
                enabled: true,
                ..policy
            };
            assert!(with_https.validate().is_ok(), "{field}");
        }
    }

    #[test]
    fn hsts_max_age_is_bounded_to_two_years() {
        let mut policy = enabled();
        policy.hsts.max_age_secs = MAX_HSTS_AGE_SECS;
        assert!(policy.validate().is_ok());
        policy.hsts.max_age_secs = 0;
        assert!(policy.validate().is_ok());
        policy.hsts.max_age_secs = MAX_HSTS_AGE_SECS + 1;
        assert_eq!(
            policy.validate().unwrap_err().field,
            "https.hsts.maxAgeSecs"
        );
    }
}
