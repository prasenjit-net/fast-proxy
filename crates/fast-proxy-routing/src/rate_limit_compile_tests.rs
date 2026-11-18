//! Compiling rate limits and body caps into the snapshot (ADR 0030).

use std::net::IpAddr;
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_limits::{Admission, RateLimitRuntime};
use fast_proxy_model::{
    Config, RateLimitHeader, RateLimitKey, RateLimitPolicy, SubsystemId, DEFAULT_IPV6_PREFIX,
};

use crate::compile::tests::{route, subsystem};
use crate::{
    compile_with, AcmeInputs, CompileInputs, CompiledRoute, MaintenanceInputs, SamplingInputs,
    Snapshot, TlsInputs,
};

const CLIENT: &str = "192.0.2.10";

fn one_per_minute(key: RateLimitKey) -> RateLimitPolicy {
    RateLimitPolicy {
        rate: 1,
        period: Duration::from_secs(60),
        burst: 1,
        key,
        ipv6_prefix: DEFAULT_IPV6_PREFIX,
    }
}

/// `shop` limits every route at one a minute per client, except `/api`, which has
/// its own policy keyed by `x-api-key`; `/static` has a 1 KiB body cap.
fn config() -> Config {
    let mut shop = subsystem(1, "shop", &["shop.example.com"]);
    shop.rate_limit = Some(one_per_minute(RateLimitKey::Client));
    let mut api = route(101, "/api", 10);
    api.rate_limit = Some(one_per_minute(RateLimitKey::Header(
        RateLimitHeader::parse("h", "x-api-key").unwrap(),
    )));
    let mut assets = route(102, "/static", 10);
    assets.max_body_bytes = Some(1024);
    shop.routes.extend([api, assets]);
    Config {
        subsystems: vec![shop, subsystem(2, "blog", &["blog.example.com"])],
    }
}

fn compile(config: &Config, limits: &RateLimitRuntime) -> Snapshot {
    compile_with(
        1,
        config,
        &RuntimeRegistry::new(),
        &CompileInputs {
            tls: &TlsInputs::off(),
            maintenance: &MaintenanceInputs::none(),
            sampling: &SamplingInputs::none(),
            acme: &AcmeInputs::none(),
            rate_limits: limits,
        },
    )
    .unwrap()
}

fn route_of<'a>(snapshot: &'a Snapshot, subsystem: i64, prefix: &str) -> &'a CompiledRoute {
    snapshot.subsystems[&SubsystemId(subsystem)]
        .routes
        .find(prefix)
        .unwrap()
        .route
}

fn check(route: &CompiledRoute, header: Option<&[u8]>) -> Admission {
    let client: IpAddr = CLIENT.parse().unwrap();
    route.rate_limit.as_ref().unwrap().check(client, header, 1)
}

#[test]
fn routes_without_their_own_policy_share_the_subsystems_budget() {
    let snapshot = compile(&config(), &RateLimitRuntime::default());
    // The seed's `/` route and `/static` both fall under the subsystem's policy.
    assert_eq!(check(route_of(&snapshot, 1, "/"), None), Admission::Allowed);
    assert!(
        matches!(
            check(route_of(&snapshot, 1, "/static"), None),
            Admission::Limited { .. }
        ),
        "one budget per client across the subsystem's routes"
    );
}

#[test]
fn a_routes_own_policy_replaces_the_subsystems() {
    let snapshot = compile(&config(), &RateLimitRuntime::default());
    let api = route_of(&snapshot, 1, "/api");
    assert_eq!(
        api.rate_limit.as_ref().unwrap().key_header(),
        Some("x-api-key")
    );
    assert_eq!(check(route_of(&snapshot, 1, "/"), None), Admission::Allowed);
    assert_eq!(
        check(api, Some(b"k1")),
        Admission::Allowed,
        "its own budget"
    );
    assert!(matches!(check(api, Some(b"k1")), Admission::Limited { .. }));
}

#[test]
fn a_subsystem_without_a_policy_and_body_caps_compile_as_set() {
    let snapshot = compile(&config(), &RateLimitRuntime::default());
    assert!(route_of(&snapshot, 2, "/").rate_limit.is_none());
    assert_eq!(route_of(&snapshot, 1, "/static").max_body_bytes, Some(1024));
    assert_eq!(route_of(&snapshot, 1, "/").max_body_bytes, None);
}

#[test]
fn a_recompile_keeps_every_clients_budget_and_applies_a_new_rate() {
    let first = compile(&config(), &RateLimitRuntime::default());
    assert_eq!(check(route_of(&first, 1, "/"), None), Admission::Allowed);

    let mut changed = config();
    changed.subsystems[0].rate_limit = Some(RateLimitPolicy {
        burst: 3,
        ..one_per_minute(RateLimitKey::Client)
    });
    let second = compile(&changed, &first.rate_limit_runtime());
    let root = route_of(&second, 1, "/");
    assert_eq!(
        check(root, None),
        Admission::Allowed,
        "the new burst applies at once"
    );
    assert_eq!(check(root, None), Admission::Allowed);
    assert!(
        matches!(check(root, None), Admission::Limited { .. }),
        "and the request made before the reload still counts"
    );
}

#[test]
fn a_removed_policy_gives_its_keys_back_once_the_old_snapshot_is_gone() {
    let first = compile(&config(), &RateLimitRuntime::new(10));
    check(route_of(&first, 1, "/"), None);
    let budget = first.rate_limit_runtime().budget().clone();
    assert_eq!(budget.used(), 1);

    let mut unlimited = config();
    unlimited.subsystems[0].rate_limit = None;
    let second = compile(&unlimited, &first.rate_limit_runtime());
    assert!(route_of(&second, 1, "/").rate_limit.is_none());
    drop(first);
    assert_eq!(budget.used(), 0);
}

#[test]
fn a_disabled_route_takes_no_table() {
    let mut config = config();
    for route in &mut config.subsystems[0].routes {
        route.enabled = route.prefix.as_str() == "/";
    }
    config.subsystems[0].routes[1].rate_limit = None;
    let snapshot = compile(&config, &RateLimitRuntime::default());
    let identities: Vec<_> = snapshot
        .subsystems
        .values()
        .flat_map(|s| s.routes.iter())
        .filter_map(|route| {
            route
                .rate_limit
                .as_ref()
                .map(|limit| limit.identity.clone())
        })
        .collect();
    assert_eq!(identities.len(), 1, "{identities:?}");
}
