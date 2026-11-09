//! Picks the endpoint for one request: extracts the consistent-hash key the
//! group asks for, runs the group's balancer, and reports the choice.

use std::cell::RefCell;
use std::net::IpAddr;

use fast_proxy_balancing::{hash_bytes, hash_ip, monotonic_ms, RequestContext, Rng, Selection};
use fast_proxy_model::HashKey;
use fast_proxy_routing::CompiledUpstream;
use http::{header, HeaderMap};

thread_local! {
    static RNG: RefCell<Rng> = RefCell::new(Rng::seeded(rand::random()));
}

/// Chooses an endpoint of `upstream` for a request from `client`, never
/// choosing `exclude`. `None` means no endpoint may serve it.
pub fn select_endpoint(
    upstream: &CompiledUpstream,
    headers: &HeaderMap,
    client: IpAddr,
    exclude: Option<usize>,
) -> Option<Selection> {
    let ctx = RequestContext {
        key_hash: upstream
            .balancer
            .hash_key()
            .and_then(|key| hash_of_key(key, headers, client)),
    };
    RNG.with(|rng| {
        upstream.balancer.select(
            &upstream.candidates,
            &ctx,
            exclude,
            &mut rng.borrow_mut(),
            monotonic_ms(),
        )
    })
}

/// The hash of the request's value for `key`, if the request has one.
fn hash_of_key(key: &HashKey, headers: &HeaderMap, client: IpAddr) -> Option<u64> {
    match key {
        HashKey::ClientIp => Some(hash_ip(client)),
        HashKey::Header(name) => headers.get(name.as_str()).map(|v| hash_bytes(v.as_bytes())),
        HashKey::Cookie(name) => cookie_value(headers, name).map(|v| hash_bytes(v.as_bytes())),
    }
}

/// The value of cookie `name` across all `Cookie` headers.
fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(cookie, _)| *cookie == name)
        .map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        map
    }

    #[test]
    fn the_client_ip_always_has_a_key() {
        let ip = "10.1.2.3".parse().unwrap();
        assert_eq!(
            hash_of_key(&HashKey::ClientIp, &HeaderMap::new(), ip),
            Some(hash_ip(ip))
        );
    }

    #[test]
    fn a_header_key_is_the_hash_of_the_value_or_nothing() {
        let ip = "10.1.2.3".parse().unwrap();
        let key = HashKey::Header("x-user".into());
        assert_eq!(
            hash_of_key(&key, &headers(&[("x-user", "alice")]), ip),
            Some(hash_bytes(b"alice"))
        );
        assert_eq!(hash_of_key(&key, &HeaderMap::new(), ip), None);
    }

    #[test]
    fn a_cookie_key_is_found_among_several_cookies_and_headers() {
        let ip = "10.1.2.3".parse().unwrap();
        let key = HashKey::Cookie("sid".into());
        let request = headers(&[("cookie", "a=1; sid=abc"), ("cookie", "b=2")]);
        assert_eq!(hash_of_key(&key, &request, ip), Some(hash_bytes(b"abc")));
        let other = headers(&[("cookie", "xsid=abc; b=2")]);
        assert_eq!(hash_of_key(&key, &other, ip), None, "names match exactly");
        assert_eq!(hash_of_key(&key, &HeaderMap::new(), ip), None);
    }
}
