use std::net::IpAddr;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};

use crate::request_id::{header_value, REQUEST_ID};

const X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
const X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
const X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

/// Hop-by-hop headers (RFC 9110 section 7.6.1), plus the legacy
/// `Proxy-Connection`, which proxies must not forward.
const HOP_BY_HOP: [HeaderName; 9] = [
    header::CONNECTION,
    HeaderName::from_static("keep-alive"),
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
    HeaderName::from_static("proxy-connection"),
];

/// Removes hop-by-hop headers, and every header the `Connection` header
/// names, so they stay between this proxy and its direct peer.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    let named: Vec<HeaderName> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|token| HeaderName::from_bytes(token.trim().as_bytes()).ok())
        .collect();
    for name in named.iter().chain(HOP_BY_HOP.iter()) {
        headers.remove(name);
    }
}

/// Whether the request asks to switch protocols (for example to WebSocket):
/// `Connection` names `upgrade` and an `Upgrade` header is present.
pub fn is_upgrade_request(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE) && connection_has_token(headers, "upgrade")
}

fn connection_has_token(headers: &HeaderMap, token: &str) -> bool {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(token))
}

/// Like [`strip_hop_by_hop`], but keeps the `Upgrade` header and states
/// `Connection: upgrade`, so a protocol switch can pass through.
pub fn strip_hop_by_hop_keeping_upgrade(headers: &mut HeaderMap) {
    let upgrade = headers.get(header::UPGRADE).cloned();
    strip_hop_by_hop(headers);
    if let Some(upgrade) = upgrade {
        headers.insert(header::UPGRADE, upgrade);
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
}

/// How the client reached us, used to build the `X-Forwarded-*` headers.
pub struct ClientInfo {
    pub ip: IpAddr,
    /// The peer is a configured trusted proxy, so its own `X-Forwarded-*`
    /// headers are believed and extended instead of overwritten.
    pub trusted: bool,
    /// `http` or `https`, as the client connected to us.
    pub scheme: &'static str,
}

/// Sets `X-Forwarded-For/Proto/Host`. Untrusted peers get their incoming
/// values overwritten, so a client can never spoof its address.
pub fn apply_forwarded(headers: &mut HeaderMap, client: &ClientInfo) {
    let original_host = headers.get(header::HOST).cloned();

    let chain = if client.trusted {
        let existing: Vec<&str> = headers
            .get_all(X_FORWARDED_FOR)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect();
        if existing.is_empty() {
            client.ip.to_string()
        } else {
            format!("{}, {}", existing.join(", "), client.ip)
        }
    } else {
        client.ip.to_string()
    };
    headers.insert(X_FORWARDED_FOR, header_value(&chain));

    if !(client.trusted && headers.contains_key(X_FORWARDED_PROTO)) {
        headers.insert(X_FORWARDED_PROTO, HeaderValue::from_static(client.scheme));
    }
    if !(client.trusted && headers.contains_key(X_FORWARDED_HOST)) {
        match original_host {
            Some(host) => headers.insert(X_FORWARDED_HOST, host),
            None => headers.remove(X_FORWARDED_HOST),
        };
    }
}

/// Gives a request that carries its host only in the URI (an HTTP/2 `:authority`,
/// or an absolute-form request line) a `Host` header, so the upstream and the
/// `X-Forwarded-Host` header see it.
pub fn ensure_host(head: &mut http::request::Parts) {
    if head.headers.contains_key(header::HOST) {
        return;
    }
    let Some(authority) = head.uri.authority() else {
        return;
    };
    if let Ok(value) = HeaderValue::from_str(authority.as_str()) {
        head.headers.insert(header::HOST, value);
    }
}

/// Applies the route's `preserve_host` choice: either keep the client's
/// `Host`, or replace it with the upstream endpoint's address.
pub fn apply_host(headers: &mut HeaderMap, preserve_host: bool, endpoint: &str) {
    if !preserve_host {
        headers.insert(header::HOST, header_value(endpoint));
    }
}

/// Stamps the request id the upstream (and later the client) sees.
pub fn apply_request_id(headers: &mut HeaderMap, request_id: &str) {
    headers.insert(REQUEST_ID, header_value(request_id));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                HeaderName::from_static(name),
                HeaderValue::from_static(value),
            );
        }
        headers
    }

    fn client(trusted: bool) -> ClientInfo {
        ClientInfo {
            ip: "203.0.113.7".parse().unwrap(),
            trusted,
            scheme: "http",
        }
    }

    fn value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
        headers.get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn strips_the_standard_hop_by_hop_headers() {
        let mut headers = map(&[
            ("connection", "keep-alive"),
            ("keep-alive", "timeout=5"),
            ("proxy-authorization", "Basic x"),
            ("proxy-authenticate", "Basic"),
            ("te", "trailers"),
            ("trailer", "x"),
            ("transfer-encoding", "chunked"),
            ("upgrade", "websocket"),
            ("proxy-connection", "keep-alive"),
            ("content-type", "text/plain"),
            ("authorization", "Bearer t"),
        ]);
        strip_hop_by_hop(&mut headers);
        assert_eq!(headers.len(), 2);
        assert!(headers.contains_key("content-type"));
        assert!(headers.contains_key("authorization"));
    }

    #[test]
    fn also_strips_headers_named_in_connection_case_insensitively() {
        let mut headers = map(&[
            ("connection", "close, X-Secret"),
            ("connection", "x-other"),
            ("x-secret", "1"),
            ("x-other", "2"),
            ("x-kept", "3"),
        ]);
        strip_hop_by_hop(&mut headers);
        assert_eq!(headers.len(), 1);
        assert_eq!(value(&headers, "x-kept"), Some("3"));
    }

    #[test]
    fn recognizes_upgrade_requests_case_insensitively() {
        let upgrade = map(&[
            ("connection", "keep-alive, Upgrade"),
            ("upgrade", "websocket"),
        ]);
        assert!(is_upgrade_request(&upgrade));
        assert!(!is_upgrade_request(&map(&[("upgrade", "websocket")])));
        assert!(!is_upgrade_request(&map(&[("connection", "upgrade")])));
        assert!(!is_upgrade_request(&map(&[("connection", "keep-alive")])));
    }

    #[test]
    fn keeping_upgrade_preserves_it_and_still_strips_the_rest() {
        let mut headers = map(&[
            ("connection", "keep-alive, Upgrade, x-private"),
            ("upgrade", "websocket"),
            ("x-private", "1"),
            ("keep-alive", "5"),
            ("sec-websocket-key", "abc"),
        ]);
        strip_hop_by_hop_keeping_upgrade(&mut headers);
        assert_eq!(value(&headers, "upgrade"), Some("websocket"));
        assert_eq!(value(&headers, "connection"), Some("upgrade"));
        assert_eq!(value(&headers, "sec-websocket-key"), Some("abc"));
        assert!(!headers.contains_key("x-private"));
        assert!(!headers.contains_key("keep-alive"));
    }

    #[test]
    fn keeping_upgrade_without_one_is_plain_stripping() {
        let mut headers = map(&[("connection", "close"), ("x-kept", "1")]);
        strip_hop_by_hop_keeping_upgrade(&mut headers);
        assert!(!headers.contains_key("connection"));
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn ignores_garbage_tokens_in_connection() {
        let mut headers = map(&[("connection", "close, , bad token!"), ("x-kept", "1")]);
        strip_hop_by_hop(&mut headers);
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn untrusted_clients_cannot_spoof_forwarded_headers() {
        let mut headers = map(&[
            ("host", "shop.example.com"),
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "evil.example"),
        ]);
        apply_forwarded(&mut headers, &client(false));
        assert_eq!(value(&headers, "x-forwarded-for"), Some("203.0.113.7"));
        assert_eq!(value(&headers, "x-forwarded-proto"), Some("http"));
        assert_eq!(
            value(&headers, "x-forwarded-host"),
            Some("shop.example.com")
        );
        assert_eq!(headers.get_all("x-forwarded-for").iter().count(), 1);
    }

    #[test]
    fn trusted_proxies_are_appended_to_and_their_proto_and_host_kept() {
        let mut headers = map(&[
            ("host", "edge.internal"),
            ("x-forwarded-for", "1.2.3.4"),
            ("x-forwarded-for", "5.6.7.8"),
            ("x-forwarded-proto", "https"),
            ("x-forwarded-host", "shop.example.com"),
        ]);
        apply_forwarded(&mut headers, &client(true));
        assert_eq!(
            value(&headers, "x-forwarded-for"),
            Some("1.2.3.4, 5.6.7.8, 203.0.113.7")
        );
        assert_eq!(value(&headers, "x-forwarded-proto"), Some("https"));
        assert_eq!(
            value(&headers, "x-forwarded-host"),
            Some("shop.example.com")
        );
    }

    #[test]
    fn a_trusted_proxy_that_sent_nothing_gets_the_defaults() {
        let mut headers = map(&[("host", "shop.example.com")]);
        apply_forwarded(&mut headers, &client(true));
        assert_eq!(value(&headers, "x-forwarded-for"), Some("203.0.113.7"));
        assert_eq!(value(&headers, "x-forwarded-proto"), Some("http"));
        assert_eq!(
            value(&headers, "x-forwarded-host"),
            Some("shop.example.com")
        );
    }

    #[test]
    fn without_a_host_no_forwarded_host_is_sent() {
        let mut headers = map(&[("x-forwarded-host", "evil.example")]);
        apply_forwarded(&mut headers, &client(false));
        assert!(!headers.contains_key("x-forwarded-host"));
    }

    #[test]
    fn a_host_carried_only_in_the_uri_becomes_a_host_header() {
        let (mut head, ()) = http::Request::builder()
            .uri("https://shop.example.com:8443/cart")
            .body(())
            .unwrap()
            .into_parts();
        ensure_host(&mut head);
        assert_eq!(value(&head.headers, "host"), Some("shop.example.com:8443"));

        let (mut kept, ()) = http::Request::builder()
            .uri("https://other.example.com/")
            .header("host", "shop.example.com")
            .body(())
            .unwrap()
            .into_parts();
        ensure_host(&mut kept);
        assert_eq!(value(&kept.headers, "host"), Some("shop.example.com"));

        let (mut neither, ()) = http::Request::builder()
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();
        ensure_host(&mut neither);
        assert!(!neither.headers.contains_key("host"));
    }

    #[test]
    fn the_scheme_comes_from_the_connection() {
        let mut headers = map(&[("host", "shop.example.com")]);
        apply_forwarded(
            &mut headers,
            &ClientInfo {
                scheme: "https",
                ..client(false)
            },
        );
        assert_eq!(value(&headers, "x-forwarded-proto"), Some("https"));
        let mut trusted = map(&[("host", "edge"), ("x-forwarded-proto", "https")]);
        apply_forwarded(&mut trusted, &client(true));
        assert_eq!(
            value(&trusted, "x-forwarded-proto"),
            Some("https"),
            "a trusted peer's value is kept"
        );
        let mut spoofed = map(&[("host", "edge"), ("x-forwarded-proto", "https")]);
        apply_forwarded(&mut spoofed, &client(false));
        assert_eq!(
            value(&spoofed, "x-forwarded-proto"),
            Some("http"),
            "an untrusted one is replaced"
        );
    }

    #[test]
    fn host_is_kept_or_rewritten_per_the_route() {
        let mut kept = map(&[("host", "shop.example.com")]);
        apply_host(&mut kept, true, "127.0.0.1:3000");
        assert_eq!(value(&kept, "host"), Some("shop.example.com"));

        let mut rewritten = map(&[("host", "shop.example.com")]);
        apply_host(&mut rewritten, false, "127.0.0.1:3000");
        assert_eq!(value(&rewritten, "host"), Some("127.0.0.1:3000"));
        assert_eq!(rewritten.get_all("host").iter().count(), 1);
    }

    #[test]
    fn the_request_id_replaces_any_incoming_one() {
        let mut headers = map(&[("x-request-id", "old")]);
        apply_request_id(&mut headers, "new");
        assert_eq!(value(&headers, "x-request-id"), Some("new"));
        assert_eq!(headers.get_all("x-request-id").iter().count(), 1);
    }
}
