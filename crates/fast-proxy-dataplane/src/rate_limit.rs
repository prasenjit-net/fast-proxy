//! Runs a route's rate-limit policy for one request (ADR 0030): after the host
//! and route matched and maintenance, before any endpoint is chosen.

use std::net::IpAddr;
use std::time::Duration;

use fast_proxy_limits::{monotonic_ns, Admission};
use fast_proxy_routing::CompiledRoute;
use http::{header, HeaderMap, HeaderValue};

/// The policy's answer for a request from `client` (the effective address),
/// or `None` when the route has no policy. A header policy is keyed by that
/// header's value, else by the client.
pub(crate) fn admit(
    route: &CompiledRoute,
    headers: &HeaderMap,
    client: IpAddr,
) -> Option<Admission> {
    let limit = route.rate_limit.as_ref()?;
    let value = limit
        .key_header()
        .and_then(|name| headers.get(name))
        .map(HeaderValue::as_bytes);
    Some(limit.check(client, value, monotonic_ns()))
}

/// `Retry-After` for a refusal: whole seconds, rounded up, at least one.
pub(crate) fn retry_after(wait: Duration) -> (header::HeaderName, HeaderValue) {
    let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    (header::RETRY_AFTER, HeaderValue::from(seconds.max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_rounds_up_to_whole_seconds() {
        let seconds = |wait| retry_after(wait).1.to_str().unwrap().to_string();
        assert_eq!(seconds(Duration::from_millis(1)), "1");
        assert_eq!(seconds(Duration::from_secs(2)), "2");
        assert_eq!(seconds(Duration::from_millis(2001)), "3");
        assert_eq!(seconds(Duration::ZERO), "1");
    }
}
