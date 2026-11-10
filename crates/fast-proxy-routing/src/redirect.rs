//! Where a plain-HTTP request should be sent instead (ADR 0012). Pure, and in
//! this crate so the data plane and the admin resolver cannot disagree.

use fast_proxy_model::RedirectMode;
use http::{StatusCode, Uri};

use crate::{dns_host, CompiledSubsystem, Snapshot};

/// Where a request that arrived on a plain-HTTP listener should go instead, and
/// with which status; `None` when it is served as usual.
///
/// A redirect needs TLS to be on for the process (a redirect to a listener that
/// does not exist would be a trap), HTTPS enabled on the subsystem and a
/// redirect mode set. The target is built only from a host that matched a
/// configured pattern and passed [`dns_host`], plus the original path and query
/// byte for byte; the port is omitted when it is 443.
pub fn redirect_target(
    snapshot: &Snapshot,
    subsystem: &CompiledSubsystem,
    host_header: &str,
    uri: &Uri,
    https_port: u16,
) -> Option<(StatusCode, String)> {
    let status = match subsystem.https.redirect {
        RedirectMode::Off => return None,
        RedirectMode::Permanent301 => StatusCode::MOVED_PERMANENTLY,
        RedirectMode::Permanent308 => StatusCode::PERMANENT_REDIRECT,
    };
    if !snapshot.tls_enabled || !subsystem.https.enabled {
        return None;
    }
    let host = dns_host(host_header)?;
    let path_and_query = uri.path_and_query().map_or("/", |value| value.as_str());
    let location = if https_port == 443 {
        format!("https://{host}{path_and_query}")
    } else {
        format!("https://{host}:{https_port}{path_and_query}")
    };
    Some((status, location))
}

#[cfg(test)]
mod tests {
    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_model::{
        Config, HostPattern, HttpsPolicy, SubsystemConfig, SubsystemId, SubsystemName,
    };

    use super::*;
    use crate::{compile_with_tls, TlsInputs};

    fn snapshot(https: HttpsPolicy, tls_enabled: bool) -> Snapshot {
        let config = Config {
            subsystems: vec![SubsystemConfig {
                id: SubsystemId(1),
                name: SubsystemName::parse("shop").unwrap(),
                enabled: true,
                description: String::new(),
                https,
                hosts: vec![HostPattern::parse("*.example.com").unwrap()],
                upstream_groups: Vec::new(),
                routes: Vec::new(),
            }],
        };
        // HTTPS is enabled without a certificate on purpose: with TLS off the
        // compile does not require one, and these tests read only the settings.
        let mut snapshot =
            compile_with_tls(1, &config, &RuntimeRegistry::new(), &TlsInputs::off()).unwrap();
        snapshot.tls_enabled = tls_enabled;
        snapshot
    }

    fn policy(redirect: RedirectMode) -> HttpsPolicy {
        HttpsPolicy {
            enabled: true,
            redirect,
            ..HttpsPolicy::default()
        }
    }

    fn target(
        snapshot: &Snapshot,
        host: &str,
        uri: &str,
        port: u16,
    ) -> Option<(StatusCode, String)> {
        let subsystem = snapshot.subsystems.values().next().unwrap();
        redirect_target(snapshot, subsystem, host, &uri.parse().unwrap(), port)
    }

    #[test]
    fn a_redirect_keeps_the_path_and_query_byte_for_byte_and_hides_port_443() {
        let snapshot = snapshot(policy(RedirectMode::Permanent308), true);
        let (status, location) = target(
            &snapshot,
            "Shop.example.com:8000",
            "/a%2Fb/%7e?x=%20&&y",
            443,
        )
        .unwrap();
        assert_eq!(status, StatusCode::PERMANENT_REDIRECT);
        assert_eq!(location, "https://shop.example.com/a%2Fb/%7e?x=%20&&y");
        let (_, location) = target(&snapshot, "shop.example.com", "/", 8443).unwrap();
        assert_eq!(location, "https://shop.example.com:8443/");
    }

    #[test]
    fn mode_301_gives_a_301() {
        let snapshot = snapshot(policy(RedirectMode::Permanent301), true);
        assert_eq!(
            target(&snapshot, "a.example.com", "/", 443).unwrap().0,
            StatusCode::MOVED_PERMANENTLY
        );
    }

    #[test]
    fn nothing_is_redirected_when_off_when_https_is_disabled_or_when_tls_is_off() {
        assert!(target(
            &snapshot(policy(RedirectMode::Off), true),
            "a.example.com",
            "/",
            443
        )
        .is_none());
        let disabled = HttpsPolicy {
            enabled: false,
            ..HttpsPolicy::default()
        };
        assert!(target(&snapshot(disabled, true), "a.example.com", "/", 443).is_none());
        assert!(target(
            &snapshot(policy(RedirectMode::Permanent308), false),
            "a.example.com",
            "/",
            443
        )
        .is_none());
    }

    #[test]
    fn a_host_with_odd_characters_is_never_put_into_a_location() {
        let snapshot = snapshot(policy(RedirectMode::Permanent308), true);
        for host in [
            "evil.com@x.example.com",
            "a/b.example.com",
            "x y.example.com",
        ] {
            assert!(target(&snapshot, host, "/", 443).is_none(), "{host}");
        }
    }
}
