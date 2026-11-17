//! What the fuzz targets share: the fixed routing snapshot the request-path
//! targets run against, a header map built from fuzzer bytes, and the process
//! config loader (included from the binary, which has no library target).

use std::net::IpAddr;
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    BypassRules, Cidr, Config, EndpointAddr, EndpointConfig, EndpointId, EndpointState,
    GroupPolicy, HostPattern, HttpsPolicy, MaintenanceSettings, MaintenanceTarget, RateLimitKey,
    RateLimitPolicy, RetryPolicy, RouteConfig, RouteId, RoutePrefix, SubsystemConfig, SubsystemId,
    SubsystemName, UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix, DEFAULT_IPV6_PREFIX,
};
use fast_proxy_routing::{compile_all, MaintenanceInputs, Snapshot, TlsInputs};
use http::{HeaderMap, HeaderName, HeaderValue};

/// The binary's `config.toml` model, compiled here as it is in the binary.
#[allow(dead_code, unused_imports)]
#[path = "../../src/config/mod.rs"]
pub mod config;

/// The endpoint every forwarded request is rewritten for.
pub fn endpoint() -> EndpointAddr {
    EndpointAddr::parse("10.0.0.1:8080").expect("a valid endpoint")
}

fn route(id: i64, prefix: &str, rewrite: Option<&str>) -> RouteConfig {
    RouteConfig {
        id: RouteId(id),
        prefix: RoutePrefix::parse(prefix).expect("a valid prefix"),
        upstream_group_id: UpstreamGroupId(id / 10),
        strip_prefix: rewrite.is_some(),
        upstream_prefix: rewrite
            .map(|to| UpstreamPrefix::parse(to).expect("a valid upstream prefix"))
            .unwrap_or_default(),
        preserve_host: true,
        connect_timeout: Duration::from_secs(5),
        response_timeout: Duration::from_secs(30),
        retry: RetryPolicy::default(),
        enabled: id % 7 != 0,
        rate_limit: None,
        max_body_bytes: None,
    }
}

fn subsystem(id: i64, name: &str, hosts: &[&str], routes: Vec<RouteConfig>) -> SubsystemConfig {
    SubsystemConfig {
        id: SubsystemId(id),
        name: SubsystemName::parse(name).expect("a valid name"),
        enabled: id != 3,
        description: String::new(),
        https: HttpsPolicy::default(),
        hosts: hosts
            .iter()
            .map(|host| HostPattern::parse(host).expect("a valid host"))
            .collect(),
        upstream_groups: vec![UpstreamGroupConfig {
            id: UpstreamGroupId(id),
            name: "backend".into(),
            policy: GroupPolicy::default(),
            endpoints: vec![EndpointConfig {
                id: EndpointId(id),
                addr: endpoint(),
                weight: 1,
                state: EndpointState::Enabled,
            }],
        }],
        routes,
        rate_limit: (id == 1).then(|| RateLimitPolicy {
            rate: 10,
            period: Duration::from_secs(1),
            burst: 10,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        }),
    }
}

/// Exact and wildcard hosts, nested prefixes with and without a rewrite, a
/// disabled route (ids divisible by 7), a disabled subsystem (3), and a
/// maintenance scope on subsystem 2 with a bypass network.
pub fn snapshot() -> Snapshot {
    let config = Config {
        subsystems: vec![
            subsystem(
                1,
                "shop",
                &["shop.example.com", "*.shop.example.com"],
                vec![
                    route(10, "/", None),
                    route(11, "/api", Some("/v2")),
                    route(12, "/api/users/", Some("/")),
                    route(14, "/static", None),
                ],
            ),
            subsystem(
                2,
                "blog",
                &["blog.example.com", "*.example.org"],
                vec![route(20, "/", None), route(21, "/admin", Some("/"))],
            ),
            subsystem(3, "old", &["old.example.com"], vec![route(30, "/", None)]),
        ],
    };
    let maintenance = MaintenanceInputs {
        scopes: vec![(
            MaintenanceTarget::Subsystem(SubsystemId(2)),
            MaintenanceSettings {
                message: "down for a moment".into(),
                body: None,
                retry_after_secs: None,
                bypass: BypassRules {
                    cidrs: vec![Cidr::parse("bypass.cidrs", "10.0.0.0/8").expect("a valid network")],
                    header: None,
                },
                expires_at_ms: None,
                set_by: "fuzz".into(),
                set_at_ms: 0,
            },
        )],
        now_ms: 0,
    };
    compile_all(
        1,
        &config,
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &maintenance,
    )
    .expect("the fixture compiles")
}

/// The header names the request path treats specially; a fuzzer name of one
/// byte below 32 picks one, so they turn up far more often than by chance.
const KNOWN_HEADERS: [&str; 14] = [
    "x-forwarded-for",
    "connection",
    "transfer-encoding",
    "content-length",
    "host",
    "upgrade",
    "te",
    "keep-alive",
    "proxy-connection",
    "trailer",
    "x-api-key",
    "authorization",
    "cookie",
    "content-type",
];

/// The hosts of [`snapshot`], picked the same way by [`host`].
const KNOWN_HOSTS: [&str; 6] = [
    "shop.example.com",
    "a.shop.example.com",
    "blog.example.com",
    "x.example.org",
    "old.example.com",
    "shop.example.com:8080",
];

/// A header map from fuzzer pairs; names or values the `http` crate refuses are
/// dropped, as hyper would refuse them on the wire.
pub fn headers(pairs: &[(Vec<u8>, Vec<u8>)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        let name = match name.as_slice() {
            [pick] if *pick < 32 => {
                HeaderName::from_static(KNOWN_HEADERS[usize::from(*pick) % KNOWN_HEADERS.len()])
            }
            raw => match HeaderName::from_bytes(raw) {
                Ok(name) => name,
                Err(_) => continue,
            },
        };
        if let Ok(value) = HeaderValue::from_bytes(value) {
            map.append(name, value);
        }
    }
    map
}

/// A `Host` header: one the fixture knows (by index) or raw fuzzer text.
pub fn host(pick: u8, raw: &str) -> String {
    if pick < 200 {
        KNOWN_HOSTS[usize::from(pick) % KNOWN_HOSTS.len()].to_string()
    } else {
        raw.to_string()
    }
}

/// An address from fuzzer bytes: IPv4 or IPv6 by the first byte.
pub fn ip(bytes: [u8; 17]) -> IpAddr {
    if bytes[0] % 2 == 0 {
        IpAddr::from([bytes[1], bytes[2], bytes[3], bytes[4]])
    } else {
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&bytes[1..]);
        IpAddr::from(octets)
    }
}
