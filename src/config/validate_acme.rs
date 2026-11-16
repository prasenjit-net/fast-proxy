//! The rules for `[data_plane.acme]`, and the warnings worth printing when it
//! is on. The section is checked even while ACME is off, so a mistake is found
//! before someone turns it on, except the allowlist, which is only required
//! (and so only checked) when ACME is enabled.

use std::time::Duration;

use fast_proxy_model::DomainAllowlist;
use hyper::Uri;

use super::acme::LETS_ENCRYPT_STAGING;
use super::{AcmeConfig, AppConfig, ConfigError};

const MAX_EMAIL_LEN: usize = 254;

/// Checks the section and how it fits with the rest of the file.
pub fn check(config: &AppConfig) -> Result<(), ConfigError> {
    let acme = &config.data_plane.acme;
    check_directory(&acme.directory)?;
    check_email(&acme.email)?;
    check_durations(acme)?;
    if let Some(path) = &acme.directory_ca {
        if !path.is_file() || std::fs::File::open(path).is_err() {
            return invalid(format!(
                "data_plane.acme.directory_ca {} is not a readable file",
                path.display()
            ));
        }
    }
    if !acme.enabled {
        return Ok(());
    }
    if !config.data_plane.tls.enabled {
        return invalid(
            "data_plane.acme.enabled needs data_plane.tls.enabled: an ACME certificate is only \
             served over HTTPS",
        );
    }
    DomainAllowlist::parse(&acme.allowed_domains)
        .map(drop)
        .map_err(|err| ConfigError::Invalid(format!("data_plane.acme.{err}")))
}

/// What an operator should know about a valid, enabled ACME setup: untrusted
/// staging certificates, and no plain listener on port 80 (the CA validates
/// HTTP-01 there; a NAT or load balancer may map it, so this is not an error).
pub fn warnings(config: &AppConfig) -> Vec<String> {
    let acme = &config.data_plane.acme;
    if !acme.enabled {
        return Vec::new();
    }
    let mut warnings = Vec::new();
    if acme.directory == LETS_ENCRYPT_STAGING {
        warnings.push(
            "ACME uses the Let's Encrypt staging directory: its certificates are not publicly \
             trusted; set data_plane.acme.directory for production"
                .to_string(),
        );
    }
    if !config
        .data_plane
        .listener
        .iter()
        .any(|listener| listener.listen.port() == 80)
    {
        warnings.push(
            "ACME is on but no [[data_plane.listener]] listens on port 80; the CA checks HTTP-01 \
             challenges on port 80, so it must be forwarded to a plain listener"
                .to_string(),
        );
    }
    warnings
}

/// The directory must be an absolute `https` URL: the account key signs every
/// request, but the CA's answers (and the issued chain) need an authenticated
/// channel. A private or test CA is trusted through `directory_ca`.
fn check_directory(directory: &str) -> Result<(), ConfigError> {
    let usable = directory
        .parse::<Uri>()
        .is_ok_and(|uri| uri.scheme_str() == Some("https") && uri.host().is_some());
    if usable {
        return Ok(());
    }
    invalid(format!(
        "data_plane.acme.directory {directory:?} must be an https URL"
    ))
}

/// An optional contact: `local@domain`, no spaces, at most 254 characters.
fn check_email(email: &str) -> Result<(), ConfigError> {
    if email.is_empty() {
        return Ok(());
    }
    let well_formed = email.len() <= MAX_EMAIL_LEN
        && !email.contains(|c: char| c.is_whitespace() || c.is_control())
        && email.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty() && domain.contains('.') && !domain.contains('@')
        });
    if well_formed {
        return Ok(());
    }
    invalid(format!(
        "data_plane.acme.email {email:?} is not an email address"
    ))
}

fn check_durations(acme: &AcmeConfig) -> Result<(), ConfigError> {
    let within = |key: &str, value: Duration, min: Duration, max: Duration, range: &str| {
        if (min..=max).contains(&value) {
            return Ok(());
        }
        invalid(format!("data_plane.acme.{key} must be {range}"))
    };
    within(
        "renew_check_interval",
        acme.renew_check_interval,
        Duration::from_secs(60),
        Duration::from_secs(86_400),
        "between 1m and 24h",
    )?;
    within(
        "lease_ttl",
        acme.lease_ttl,
        Duration::from_secs(30),
        Duration::from_secs(3600),
        "between 30s and 1h",
    )?;
    if let Some(wait) = acme.propagation_wait {
        within(
            "propagation_wait",
            wait,
            Duration::ZERO,
            Duration::from_secs(600),
            "at most 10m",
        )?;
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> Result<(), ConfigError> {
    Err(ConfigError::Invalid(message.into()))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use super::*;

    fn enabled() -> AppConfig {
        let mut config = AppConfig::default();
        config.data_plane.tls.enabled = true;
        config.data_plane.acme.enabled = true;
        config.data_plane.acme.allowed_domains = vec!["*.example.com".into()];
        config
    }

    fn message(config: &AppConfig) -> String {
        check(config).unwrap_err().to_string()
    }

    #[test]
    fn the_defaults_and_an_enabled_setup_are_valid() {
        check(&AppConfig::default()).unwrap();
        check(&enabled()).unwrap();
    }

    #[test]
    fn acme_needs_tls_and_an_allowlist_only_when_enabled() {
        let mut config = enabled();
        config.data_plane.tls.enabled = false;
        assert!(message(&config).contains("data_plane.tls.enabled"));

        let mut config = enabled();
        config.data_plane.acme.allowed_domains.clear();
        assert!(message(&config).contains("data_plane.acme.allowed_domains"));
        config.data_plane.acme.allowed_domains = vec!["bad name".into()];
        assert!(message(&config).contains("bad name"));

        config.data_plane.acme.enabled = false;
        check(&config).unwrap();
    }

    #[test]
    fn the_directory_must_be_https() {
        for good in [
            "https://acme-v02.api.letsencrypt.org/directory",
            "https://localhost:14000/dir",
            "https://127.0.0.1:14000/dir",
        ] {
            let mut config = AppConfig::default();
            config.data_plane.acme.directory = good.into();
            check(&config).unwrap_or_else(|err| panic!("{good}: {err}"));
        }
        for bad in [
            "",
            "not a url",
            "/dir",
            "http://ca.example/dir",
            "http://127.0.0.1:14000/dir",
            "ftp://ca.example/dir",
        ] {
            let mut config = AppConfig::default();
            config.data_plane.acme.directory = bad.into();
            assert!(
                message(&config).contains("data_plane.acme.directory"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_email_is_optional_but_must_look_like_one() {
        let mut config = AppConfig::default();
        config.data_plane.acme.email = "ops@example.com".into();
        check(&config).unwrap();
        for bad in [
            "ops",
            "@example.com",
            "ops@localhost",
            "o ps@example.com",
            "a@b@c.com",
        ] {
            config.data_plane.acme.email = bad.into();
            assert!(
                message(&config).contains("data_plane.acme.email"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn durations_are_bounded() {
        type BreakIt = fn(&mut AcmeConfig);
        let cases: [(BreakIt, &str); 5] = [
            (
                |a| a.renew_check_interval = Duration::from_secs(59),
                "renew_check_interval",
            ),
            (
                |a| a.renew_check_interval = Duration::from_secs(86_401),
                "renew_check_interval",
            ),
            (|a| a.lease_ttl = Duration::from_secs(29), "lease_ttl"),
            (|a| a.lease_ttl = Duration::from_secs(3601), "lease_ttl"),
            (
                |a| a.propagation_wait = Some(Duration::from_secs(601)),
                "propagation_wait",
            ),
        ];
        for (break_it, key) in cases {
            let mut config = AppConfig::default();
            break_it(&mut config.data_plane.acme);
            assert!(message(&config).contains(key), "{key}");
        }
        let mut config = AppConfig::default();
        config.data_plane.acme.propagation_wait = Some(Duration::ZERO);
        check(&config).unwrap();
    }

    #[test]
    fn the_directory_ca_must_be_a_readable_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = AppConfig::default();
        config.data_plane.acme.directory_ca = Some(dir.path().to_path_buf());
        assert!(message(&config).contains("directory_ca"));
        config.data_plane.acme.directory_ca = Some(dir.path().join("missing.pem"));
        assert!(message(&config).contains("directory_ca"));
        let file = dir.path().join("ca.pem");
        std::fs::write(&file, b"pem").unwrap();
        config.data_plane.acme.directory_ca = Some(file);
        check(&config).unwrap();
    }

    #[test]
    fn warnings_name_staging_and_a_missing_port_80_listener() {
        assert!(
            warnings(&AppConfig::default()).is_empty(),
            "off: no warnings"
        );
        let config = enabled();
        let found = warnings(&config);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("staging"));
        assert!(found[1].contains("port 80"));

        let mut production = enabled();
        production.data_plane.acme.directory =
            "https://acme-v02.api.letsencrypt.org/directory".into();
        production.data_plane.listener[0].listen = SocketAddr::from(([0, 0, 0, 0], 80));
        assert!(warnings(&production).is_empty());
    }
}
