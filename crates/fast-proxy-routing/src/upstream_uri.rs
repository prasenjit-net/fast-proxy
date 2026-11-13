use http::uri::{Authority, PathAndQuery, Scheme};
use http::Uri;

use fast_proxy_model::EndpointAddr;

use crate::CompiledRoute;

/// Builds the URI to request from the chosen `endpoint`: its address, the
/// rewritten path, and the original query string byte for byte. The scheme
/// is `https` when the route's upstream group has TLS on.
///
/// With `strip_prefix` the path the route matched on is replaced by the
/// route's `upstream_prefix`; without it the whole original path is kept
/// and `upstream_prefix` is simply put in front.
pub fn build_upstream_uri(
    route: &CompiledRoute,
    endpoint: &EndpointAddr,
    original: &Uri,
    consumed_prefix_len: usize,
) -> Option<Uri> {
    let path = original.path();
    let kept = if route.strip_prefix {
        &path[consumed_prefix_len..]
    } else {
        path
    };
    let mut rewritten = format!("{}{kept}", route.upstream_prefix.as_str());
    if !rewritten.starts_with('/') {
        rewritten.insert(0, '/');
    }
    if let Some(query) = original.query() {
        rewritten.push('?');
        rewritten.push_str(query);
    }

    let authority: Authority = endpoint.to_string().parse().ok()?;
    let path_and_query: PathAndQuery = rewritten.parse().ok()?;
    let scheme = if route.upstream.policy.tls.enabled {
        Scheme::HTTPS
    } else {
        Scheme::HTTP
    };
    Uri::builder()
        .scheme(scheme)
        .authority(authority)
        .path_and_query(path_and_query)
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_model::{
        EndpointConfig, EndpointId, EndpointState, FailurePolicy, GroupPolicy, RetryPolicy,
        RouteId, UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix,
    };

    use super::*;
    use crate::CompiledUpstream;

    fn endpoint() -> EndpointAddr {
        EndpointAddr::parse("127.0.0.1:3000").unwrap()
    }

    fn route(strip_prefix: bool, upstream_prefix: &str) -> CompiledRoute {
        CompiledRoute {
            id: RouteId(1),
            prefix: "/".into(),
            strip_prefix,
            upstream_prefix: UpstreamPrefix::parse(upstream_prefix).unwrap(),
            preserve_host: true,
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(30),
            retry: RetryPolicy::default(),
            upstream: Arc::new(
                CompiledUpstream::build(
                    &UpstreamGroupConfig {
                        id: UpstreamGroupId(1),
                        name: "backend".into(),
                        policy: GroupPolicy {
                            failure: FailurePolicy::FailOpen,
                            ..GroupPolicy::default()
                        },
                        endpoints: vec![EndpointConfig {
                            id: EndpointId(1),
                            addr: endpoint(),
                            weight: 1,
                            state: EndpointState::Enabled,
                        }],
                    },
                    &RuntimeRegistry::new().rebuilt_for([(EndpointId(1), &endpoint())]),
                    None,
                )
                .unwrap(),
            ),
            maintenance: None,
            sampling: crate::CompiledSampling {
                policy: Arc::default(),
                latency: Arc::default(),
            },
        }
    }

    fn rewrite(strip: bool, upstream_prefix: &str, request: &str, consumed: usize) -> String {
        let uri: Uri = request.parse().unwrap();
        build_upstream_uri(&route(strip, upstream_prefix), &endpoint(), &uri, consumed)
            .unwrap()
            .to_string()
    }

    #[test]
    fn keeps_the_path_and_query_when_nothing_is_rewritten() {
        assert_eq!(
            rewrite(false, "", "/api/users?x=1&y=%20z", 4),
            "http://127.0.0.1:3000/api/users?x=1&y=%20z"
        );
    }

    #[test]
    fn strip_prefix_removes_the_matched_prefix() {
        assert_eq!(
            rewrite(true, "", "/api/users?x=1", 4),
            "http://127.0.0.1:3000/users?x=1"
        );
        assert_eq!(rewrite(true, "", "/api", 4), "http://127.0.0.1:3000/");
        assert_eq!(rewrite(true, "", "/api/", 4), "http://127.0.0.1:3000/");
    }

    #[test]
    fn upstream_prefix_is_prepended() {
        assert_eq!(
            rewrite(false, "/v2", "/api/x", 4),
            "http://127.0.0.1:3000/v2/api/x"
        );
        assert_eq!(
            rewrite(true, "/v2", "/api/x", 4),
            "http://127.0.0.1:3000/v2/x"
        );
        assert_eq!(rewrite(true, "/v2", "/api", 4), "http://127.0.0.1:3000/v2");
    }

    #[test]
    fn the_root_route_consumes_nothing() {
        assert_eq!(rewrite(true, "", "/a/b", 0), "http://127.0.0.1:3000/a/b");
        assert_eq!(rewrite(true, "/v2", "/", 0), "http://127.0.0.1:3000/v2/");
    }

    #[test]
    fn the_query_is_preserved_byte_for_byte() {
        assert_eq!(
            rewrite(false, "", "/p?a=%2F&b=%zz&&c", 0),
            "http://127.0.0.1:3000/p?a=%2F&b=%zz&&c"
        );
        assert_eq!(rewrite(false, "", "/p?", 0), "http://127.0.0.1:3000/p?");
        assert_eq!(rewrite(true, "", "/?a=b", 0), "http://127.0.0.1:3000/?a=b");
    }

    #[test]
    fn encoded_and_dotted_paths_are_forwarded_unmodified() {
        assert_eq!(
            rewrite(false, "", "/a%2Fb/%2e%2e/c", 0),
            "http://127.0.0.1:3000/a%2Fb/%2e%2e/c"
        );
    }

    #[test]
    fn a_tls_group_gets_an_https_scheme_with_and_without_a_port() {
        let mut tls_route = route(false, "");
        let mut group = Arc::try_unwrap(tls_route.upstream).unwrap();
        group.policy.tls.enabled = true;
        tls_route.upstream = Arc::new(group);
        let uri: Uri = "/x?q=1".parse().unwrap();
        let with_port = build_upstream_uri(&tls_route, &endpoint(), &uri, 0).unwrap();
        assert_eq!(with_port.to_string(), "https://127.0.0.1:3000/x?q=1");
        let ipv6 = EndpointAddr::parse("[::1]:8443").unwrap();
        assert_eq!(
            build_upstream_uri(&tls_route, &ipv6, &uri, 0)
                .unwrap()
                .to_string(),
            "https://[::1]:8443/x?q=1"
        );
        let default_port = EndpointAddr::parse("backend.internal:443").unwrap();
        assert_eq!(
            build_upstream_uri(&tls_route, &default_port, &uri, 0)
                .unwrap()
                .to_string(),
            "https://backend.internal:443/x?q=1"
        );
    }

    #[test]
    fn ipv6_endpoints_keep_their_brackets() {
        let route = route(false, "");
        let uri: Uri = "/x".parse().unwrap();
        let ipv6 = EndpointAddr::parse("[::1]:8080").unwrap();
        assert_eq!(
            build_upstream_uri(&route, &ipv6, &uri, 0)
                .unwrap()
                .to_string(),
            "http://[::1]:8080/x"
        );
    }
}
