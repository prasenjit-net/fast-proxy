//! Maintenance in the request path (ADR 0018): deciding whether a scope answers
//! or lets a request through, and the answer itself. The decision is the pure
//! `CompiledMaintenance::check` of the routing crate, shared with the admin
//! resolver; nothing here reads an endpoint, a retry budget or health state.

use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use fast_proxy_routing::{CompiledMaintenance, MaintenanceCheck, MaintenanceScope};
use http::{header, HeaderMap, HeaderName, HeaderValue, Response, StatusCode};

use crate::request_id::{header_value, REQUEST_ID};
use crate::responses::{boxed, response_format};
use crate::ResponseBody;

/// Tells the operator a request passed through maintenance by a bypass rule.
pub(crate) const BYPASS_HEADER: HeaderName = HeaderName::from_static("x-maintenance-bypass");

/// The scopes a request passed through by a bypass rule, and the request
/// headers that carry a bypass secret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BypassedScopes {
    passed: Vec<MaintenanceScope>,
    /// The bypass header of every scope that applied, whether or not it matched.
    secret_headers: Vec<HeaderName>,
}

impl BypassedScopes {
    pub(crate) fn scopes(&self) -> &[MaintenanceScope] {
        &self.passed
    }

    /// Removes the bypass headers from a request about to be forwarded: the
    /// secret is for this proxy, and the upstream (and its logs) must never see
    /// it, nor a wrong guess at it.
    pub(crate) fn strip_secrets(&self, headers: &mut HeaderMap) {
        for name in &self.secret_headers {
            headers.remove(name);
        }
    }

    /// `global`, `subsystem,route`, ...: the scopes passed, for the response header.
    pub(crate) fn header_value(&self) -> Option<HeaderValue> {
        if self.passed.is_empty() {
            return None;
        }
        let labels: Vec<&str> = self.passed.iter().map(|scope| scope.label()).collect();
        HeaderValue::from_str(&labels.join(",")).ok()
    }
}

/// What the scopes that apply to a request decided.
pub(crate) enum ScopeOutcome<'snapshot> {
    /// Serve the request, having passed these scopes by a bypass rule.
    Pass(BypassedScopes),
    /// This scope answers the request.
    Respond(&'snapshot CompiledMaintenance),
}

/// Checks `scopes` (global, subsystem or route, in that order) for one request.
/// Any scope that is on and does not let the request through answers it. The
/// clock is read only when a scope exists, so a proxy with nothing in
/// maintenance does no extra work.
pub(crate) fn evaluate<'snapshot>(
    scopes: &[Option<&'snapshot CompiledMaintenance>],
    headers: &HeaderMap,
    client: IpAddr,
    mut bypassed: BypassedScopes,
) -> ScopeOutcome<'snapshot> {
    let mut now = None;
    for scope in scopes.iter().flatten() {
        let now_ms = *now.get_or_insert_with(now_ms);
        let header_name = scope.bypass_header_name();
        let presented = header_name
            .and_then(|name| headers.get(name))
            .and_then(|value| value.to_str().ok());
        match scope.check(now_ms, client, presented) {
            MaintenanceCheck::Inactive => {}
            MaintenanceCheck::Bypassed => bypassed.passed.push(scope.scope),
            MaintenanceCheck::Respond => return ScopeOutcome::Respond(scope),
        }
        if let Some(name) =
            header_name.and_then(|name| HeaderName::from_bytes(name.as_bytes()).ok())
        {
            bypassed.secret_headers.push(name);
        }
    }
    ScopeOutcome::Pass(bypassed)
}

/// The 503 a scope answers with: `Retry-After`, no caching, the request id, and
/// a body chosen by the client's `Accept` (or the operator's own).
pub(crate) fn render(
    scope: &CompiledMaintenance,
    request_headers: &HeaderMap,
    request_id: &str,
) -> Response<ResponseBody> {
    let now = now_ms();
    let body = scope.render(response_format(request_headers), now);
    let mut response = Response::new(boxed(body.text));
    *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(body.content_type),
    );
    headers.insert(
        header::RETRY_AFTER,
        HeaderValue::from(scope.retry_after(now)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // The operator chose the content type; a browser must not second-guess it.
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(REQUEST_ID, header_value(request_id));
    response
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{BypassRules, MaintenanceSettings};

    use super::*;

    fn scope(expires_at_ms: Option<i64>, cidr: Option<&str>) -> CompiledMaintenance {
        let mut settings = MaintenanceSettings {
            message: "x".into(),
            body: None,
            retry_after_secs: None,
            bypass: BypassRules::default(),
            expires_at_ms,
            set_by: "ops".into(),
            set_at_ms: 0,
        };
        if let Some(cidr) = cidr {
            settings.bypass.cidrs =
                vec![fast_proxy_model::Cidr::parse("bypass.cidrs", cidr).unwrap()];
        }
        CompiledMaintenance::new(MaintenanceScope::Global, settings)
    }

    fn client() -> IpAddr {
        "203.0.113.9".parse().unwrap()
    }

    #[test]
    fn with_no_scopes_a_request_passes_and_bypasses_nothing() {
        let outcome = evaluate(
            &[None, None, None],
            &HeaderMap::new(),
            client(),
            BypassedScopes::default(),
        );
        assert!(matches!(outcome, ScopeOutcome::Pass(bypassed) if bypassed.scopes().is_empty()));
    }

    #[test]
    fn an_expired_scope_passes_without_counting_as_a_bypass() {
        let expired = scope(Some(1), None);
        let outcome = evaluate(
            &[Some(&expired)],
            &HeaderMap::new(),
            client(),
            BypassedScopes::default(),
        );
        assert!(matches!(outcome, ScopeOutcome::Pass(bypassed) if bypassed.scopes().is_empty()));
    }

    #[test]
    fn the_first_scope_that_answers_wins_even_after_one_was_passed() {
        let passing = scope(None, Some("203.0.113.0/24"));
        let answering = scope(None, None);
        let outcome = evaluate(
            &[Some(&passing), Some(&answering)],
            &HeaderMap::new(),
            client(),
            BypassedScopes::default(),
        );
        assert!(matches!(outcome, ScopeOutcome::Respond(_)));
    }

    #[test]
    fn passed_scopes_are_named_in_order_for_the_response_header() {
        let mut bypassed = BypassedScopes::default();
        assert!(bypassed.header_value().is_none());
        bypassed.passed.push(MaintenanceScope::Global);
        bypassed.passed.push(MaintenanceScope::Route);
        assert_eq!(bypassed.header_value().unwrap(), "global,route");
    }
}
