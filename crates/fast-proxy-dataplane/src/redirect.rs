//! The redirect response and the HSTS header (ADR 0012). Whether to redirect and
//! where to is decided in `fast-proxy-routing`, shared with the admin resolver.

use bytes::Bytes;
use fast_proxy_model::Hsts;
use fast_proxy_routing::{CompiledSubsystem, Snapshot};
use http::{header, HeaderValue, Response, StatusCode};
use http_body_util::{BodyExt, Full};

use crate::request_id::{header_value, REQUEST_ID};
use crate::ResponseBody;

/// The redirect response.
pub(crate) fn render_redirect(
    status: StatusCode,
    location: &str,
    request_id: &str,
) -> Option<Response<ResponseBody>> {
    let mut response = Response::new(
        Full::new(Bytes::new())
            .map_err(|never| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(header::LOCATION, HeaderValue::from_str(location).ok()?);
    headers.insert(REQUEST_ID, header_value(request_id));
    Some(response)
}

/// The `Strict-Transport-Security` value for a subsystem's HTTPS responses, or
/// `None` when it is off. Only ever sent on HTTPS (RFC 6797).
pub(crate) fn hsts_value(
    snapshot: &Snapshot,
    subsystem: &CompiledSubsystem,
) -> Option<HeaderValue> {
    let Hsts {
        enabled,
        max_age_secs,
        include_subdomains,
    } = subsystem.https.hsts;
    if !(snapshot.tls_enabled && subsystem.https.enabled && enabled) {
        return None;
    }
    let subdomains = if include_subdomains {
        "; includeSubDomains"
    } else {
        ""
    };
    HeaderValue::from_str(&format!("max-age={max_age_secs}{subdomains}")).ok()
}

#[cfg(test)]
mod tests {
    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_model::{
        Config, HostPattern, HttpsPolicy, RedirectMode, SubsystemConfig, SubsystemId, SubsystemName,
    };
    use fast_proxy_routing::{compile_with_tls, TlsInputs};

    use super::*;

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

    #[test]
    fn hsts_needs_https_enabled_hsts_on_and_tls_on() {
        let on = HttpsPolicy {
            hsts: Hsts {
                enabled: true,
                max_age_secs: 600,
                include_subdomains: false,
            },
            ..policy(RedirectMode::Off)
        };
        let value = |snapshot: &Snapshot| {
            hsts_value(snapshot, snapshot.subsystems.values().next().unwrap())
        };
        assert_eq!(value(&snapshot(on, true)).unwrap(), "max-age=600");
        let with_subdomains = HttpsPolicy {
            hsts: Hsts {
                include_subdomains: true,
                ..on.hsts
            },
            ..on
        };
        assert_eq!(
            value(&snapshot(with_subdomains, true)).unwrap(),
            "max-age=600; includeSubDomains"
        );
        assert!(value(&snapshot(on, false)).is_none(), "TLS off");
        assert!(
            value(&snapshot(policy(RedirectMode::Off), true)).is_none(),
            "HSTS off"
        );
    }

    #[test]
    fn the_response_carries_the_location_and_request_id() {
        let response = render_redirect(
            StatusCode::PERMANENT_REDIRECT,
            "https://a.example.com/x",
            "rid",
        )
        .unwrap();
        assert_eq!(
            response.headers()[header::LOCATION],
            "https://a.example.com/x"
        );
        assert_eq!(response.headers()["x-request-id"], "rid");
    }
}
