use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{Config, RoutePrefix, SubsystemConfig, UpstreamGroupId};

use crate::compiled_upstream::GroupFault;
use crate::snapshot::{CompiledRoute, CompiledSubsystem};
use crate::{
    CompileError, CompiledHttps, CompiledUpstream, HostTable, RouteTable, Snapshot, TlsInputs,
};

/// Compiles `config` into a snapshot labelled `version`. Pure and
/// deterministic given `previous`; fails on any violated cross-table rule.
///
/// An endpoint whose id and address are unchanged keeps its runtime state
/// from `previous` (health, ejection, latency); a changed address or a new
/// endpoint starts fresh. Pass the live snapshot's
/// [`Snapshot::runtime_registry`], or an empty registry for a first load.
pub fn compile(
    version: u64,
    config: &Config,
    previous: &RuntimeRegistry,
) -> Result<Snapshot, CompileError> {
    compile_with_tls(version, config, previous, &TlsInputs::off())
}

/// Like [`compile`], carrying the TLS state into the snapshot: each subsystem
/// gets its HTTPS settings and certificate handle, and the compile fails when
/// an upstream names a CA bundle that does not exist or, while TLS is on, a
/// subsystem with HTTPS enabled has no certificate or references a missing one.
/// A certificate that exists but cannot be read is not an error: the
/// subsystem's handshakes fall back to the placeholder certificate.
pub fn compile_with_tls(
    version: u64,
    config: &Config,
    previous: &RuntimeRegistry,
    tls: &TlsInputs,
) -> Result<Snapshot, CompileError> {
    let registry = previous.rebuilt_for(
        config
            .subsystems
            .iter()
            .flat_map(|subsystem| &subsystem.upstream_groups)
            .flat_map(|group| &group.endpoints)
            .map(|endpoint| (endpoint.id, &endpoint.addr)),
    );
    let mut host_owners: HashMap<String, &SubsystemConfig> = HashMap::new();
    let mut seen_ids = HashSet::new();
    let mut seen_names = HashSet::new();
    let mut host_entries = Vec::new();
    let mut subsystems = HashMap::new();

    for subsystem in &config.subsystems {
        let name = subsystem.name.as_str();
        if !seen_ids.insert(subsystem.id) || !seen_names.insert(name) {
            return Err(CompileError::DuplicateSubsystem(name.to_string()));
        }
        for pattern in &subsystem.hosts {
            let key = pattern.to_pattern_string();
            if let Some(first) = host_owners.insert(key.clone(), subsystem) {
                return Err(CompileError::DuplicateHost {
                    pattern: key,
                    first: first.name.as_str().to_string(),
                    second: name.to_string(),
                });
            }
            host_entries.push((pattern, subsystem.id));
        }
        subsystems.insert(subsystem.id, compile_subsystem(subsystem, &registry, tls)?);
    }

    Ok(Snapshot {
        version,
        tls_enabled: tls.enabled,
        hosts: HostTable::build(host_entries),
        subsystems,
    })
}

fn compile_subsystem(
    subsystem: &SubsystemConfig,
    registry: &RuntimeRegistry,
    tls: &TlsInputs,
) -> Result<CompiledSubsystem, CompileError> {
    let name = subsystem.name.as_str();
    let https = compile_https(subsystem, tls)?;
    let groups = compile_groups(subsystem, registry, tls)?;

    let mut seen_prefixes = HashSet::new();
    let mut routes: Vec<(RoutePrefix, CompiledRoute)> = Vec::new();
    for route in &subsystem.routes {
        if !seen_prefixes.insert(route.prefix.as_str()) {
            return Err(CompileError::DuplicateRoutePrefix {
                subsystem: name.to_string(),
                prefix: route.prefix.as_str().to_string(),
            });
        }
        let group =
            groups
                .get(&route.upstream_group_id)
                .ok_or_else(|| CompileError::UnknownUpstream {
                    subsystem: name.to_string(),
                    prefix: route.prefix.as_str().to_string(),
                })?;
        if !route.enabled {
            continue;
        }
        routes.push((
            route.prefix.clone(),
            CompiledRoute {
                id: route.id,
                prefix: route.prefix.as_str().to_string(),
                strip_prefix: route.strip_prefix,
                upstream_prefix: route.upstream_prefix.clone(),
                preserve_host: route.preserve_host,
                connect_timeout: route.connect_timeout,
                response_timeout: route.response_timeout,
                retry: route.retry,
                upstream: Arc::clone(group),
            },
        ));
    }

    Ok(CompiledSubsystem {
        name: subsystem.name.clone(),
        enabled: subsystem.enabled,
        https,
        routes: RouteTable::build(routes),
        upstreams: groups,
    })
}

/// Resolves the subsystem's certificate. Checked only while TLS is on: with it
/// off the settings are inert.
fn compile_https(
    subsystem: &SubsystemConfig,
    tls: &TlsInputs,
) -> Result<CompiledHttps, CompileError> {
    let policy = &subsystem.https;
    if !tls.enabled || !policy.enabled {
        return Ok(CompiledHttps::new(policy, None));
    }
    let subsystem_name = subsystem.name.as_str().to_string();
    let Some(id) = policy.certificate_id else {
        return Err(CompileError::HttpsWithoutCertificate {
            subsystem: subsystem_name,
        });
    };
    if tls.certificates.entry(id).is_none() {
        return Err(CompileError::UnknownCertificate {
            subsystem: subsystem_name,
            id: id.0,
        });
    }
    Ok(CompiledHttps::new(
        policy,
        tls.certificates.handle(id).cloned(),
    ))
}

/// Compiles a subsystem's upstream groups, enforcing unique names and each
/// group's own rules (endpoint count, unique addresses, valid policy).
fn compile_groups(
    subsystem: &SubsystemConfig,
    registry: &RuntimeRegistry,
    tls: &TlsInputs,
) -> Result<HashMap<UpstreamGroupId, Arc<CompiledUpstream>>, CompileError> {
    let name = subsystem.name.as_str();
    let mut by_id = HashMap::new();
    let mut seen_names = HashSet::new();
    for group in &subsystem.upstream_groups {
        if !seen_names.insert(group.name.as_str()) {
            return Err(CompileError::DuplicateUpstreamName {
                subsystem: name.to_string(),
                name: group.name.clone(),
            });
        }
        let ca = &group.policy.tls.ca;
        if !ca.is_empty() && !tls.ca_bundles.contains(ca) {
            return Err(CompileError::UnknownCaBundle {
                subsystem: name.to_string(),
                upstream: group.name.clone(),
                name: ca.clone(),
            });
        }
        let compiled = CompiledUpstream::build(group, registry).map_err(|fault| {
            let (subsystem, upstream) = (name.to_string(), group.name.clone());
            match fault {
                GroupFault::EndpointCount(count) => CompileError::EndpointCount {
                    subsystem,
                    upstream,
                    count,
                },
                GroupFault::DuplicateEndpoint(addr) => CompileError::DuplicateEndpoint {
                    subsystem,
                    upstream,
                    addr,
                },
                GroupFault::Invalid(reason) => CompileError::InvalidUpstream {
                    subsystem,
                    upstream,
                    reason,
                },
            }
        })?;
        by_id.insert(group.id, Arc::new(compiled));
    }
    Ok(by_id)
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use crate::Lookup;
    use fast_proxy_model::{
        EndpointAddr, EndpointConfig, EndpointId, EndpointState, GroupPolicy, HostPattern,
        RetryPolicy, RouteConfig, RouteId, SubsystemId, SubsystemName, UpstreamGroupConfig,
        UpstreamPrefix,
    };

    use super::*;

    /// Compiles with nothing carried over.
    fn compile(version: u64, config: &Config) -> Result<Snapshot, CompileError> {
        super::compile(version, config, &RuntimeRegistry::new())
    }

    pub(crate) fn group(id: i64, name: &str, addr: &str) -> UpstreamGroupConfig {
        UpstreamGroupConfig {
            id: UpstreamGroupId(id),
            name: name.into(),
            policy: GroupPolicy::default(),
            endpoints: vec![EndpointConfig {
                id: EndpointId(id),
                addr: EndpointAddr::parse(addr).unwrap(),
                weight: 1,
                state: EndpointState::Enabled,
            }],
        }
    }

    pub(crate) fn route(id: i64, prefix: &str, group_id: i64) -> RouteConfig {
        RouteConfig {
            id: RouteId(id),
            prefix: RoutePrefix::parse(prefix).unwrap(),
            upstream_group_id: UpstreamGroupId(group_id),
            strip_prefix: false,
            upstream_prefix: UpstreamPrefix::default(),
            preserve_host: true,
            connect_timeout: Duration::from_secs(5),
            response_timeout: Duration::from_secs(30),
            retry: RetryPolicy::default(),
            enabled: true,
        }
    }

    pub(crate) fn subsystem(id: i64, name: &str, hosts: &[&str]) -> SubsystemConfig {
        SubsystemConfig {
            id: SubsystemId(id),
            name: SubsystemName::parse(name).unwrap(),
            enabled: true,
            description: String::new(),
            https: fast_proxy_model::HttpsPolicy::default(),
            hosts: hosts
                .iter()
                .map(|h| HostPattern::parse(h).unwrap())
                .collect(),
            upstream_groups: vec![group(id * 10, "backend", "127.0.0.1:3000")],
            routes: vec![route(id * 100, "/", id * 10)],
        }
    }

    fn config(subsystems: Vec<SubsystemConfig>) -> Config {
        Config { subsystems }
    }

    #[test]
    fn an_empty_config_compiles_to_an_empty_snapshot() {
        let snapshot = compile(7, &Config::default()).unwrap();
        assert_eq!(snapshot.version, 7);
        assert!(snapshot.subsystems.is_empty());
        assert_eq!(snapshot.hosts.lookup("example.com"), None);
    }

    #[test]
    fn resolves_the_endpoint_and_route_settings() {
        let mut shop = subsystem(1, "shop", &["shop.example.com"]);
        shop.routes[0].strip_prefix = true;
        shop.routes[0].upstream_prefix = UpstreamPrefix::parse("/v2").unwrap();
        shop.routes[0].preserve_host = false;
        let snapshot = compile(1, &config(vec![shop])).unwrap();

        let compiled = &snapshot.subsystems[&SubsystemId(1)];
        let found = compiled.routes.find("/x").unwrap();
        assert_eq!(
            found.route.upstream.endpoints[0].addr.to_string(),
            "127.0.0.1:3000"
        );
        assert_eq!(found.route.upstream.name, "backend");
        assert!(found.route.strip_prefix);
        assert!(!found.route.preserve_host);
        assert_eq!(found.route.upstream_prefix.as_str(), "/v2");
        assert_eq!(found.route.connect_timeout, Duration::from_secs(5));
    }

    #[test]
    fn disabled_routes_are_skipped_but_disabled_subsystems_are_kept() {
        let mut shop = subsystem(1, "shop", &["shop.example.com"]);
        shop.routes.push(route(2, "/off", 10));
        shop.routes[1].enabled = false;
        shop.enabled = false;
        let snapshot = compile(1, &config(vec![shop])).unwrap();

        assert_eq!(
            snapshot.hosts.lookup("shop.example.com"),
            Some(SubsystemId(1))
        );
        let compiled = &snapshot.subsystems[&SubsystemId(1)];
        assert!(!compiled.enabled);
        assert_eq!(compiled.routes.len(), 1);
        assert_eq!(
            compiled.routes.find("/off/x").unwrap().route.id,
            RouteId(100)
        );
    }

    #[test]
    fn compilation_is_deterministic() {
        let config = config(vec![
            subsystem(1, "shop", &["shop.example.com"]),
            subsystem(2, "blog", &["*.blog.example.com"]),
        ]);
        let first = compile(3, &config).unwrap();
        let second = compile(3, &config).unwrap();
        for host in ["shop.example.com", "a.blog.example.com", "other.com"] {
            assert_eq!(first.hosts.lookup(host), second.hosts.lookup(host));
        }
        assert_eq!(first.subsystems.len(), second.subsystems.len());
    }

    #[test]
    fn rejects_duplicate_subsystems_by_name_or_id() {
        let same_name = config(vec![subsystem(1, "shop", &[]), subsystem(2, "shop", &[])]);
        assert!(matches!(
            compile(1, &same_name),
            Err(CompileError::DuplicateSubsystem(_))
        ));
        let same_id = config(vec![subsystem(1, "shop", &[]), subsystem(1, "blog", &[])]);
        assert!(matches!(
            compile(1, &same_id),
            Err(CompileError::DuplicateSubsystem(_))
        ));
    }

    #[test]
    fn rejects_a_host_pattern_used_by_two_subsystems() {
        let config = config(vec![
            subsystem(1, "shop", &["a.example.com"]),
            subsystem(2, "blog", &["a.example.com"]),
        ]);
        let err = compile(1, &config).unwrap_err();
        assert_eq!(
            err,
            CompileError::DuplicateHost {
                pattern: "a.example.com".into(),
                first: "shop".into(),
                second: "blog".into(),
            }
        );
    }

    #[test]
    fn an_exact_and_a_wildcard_pattern_do_not_conflict() {
        let config = config(vec![
            subsystem(1, "shop", &["a.example.com"]),
            subsystem(2, "blog", &["*.example.com"]),
        ]);
        assert!(compile(1, &config).is_ok());
    }

    #[test]
    fn rejects_duplicate_upstream_names_and_route_prefixes() {
        let mut dup_group = subsystem(1, "shop", &[]);
        dup_group
            .upstream_groups
            .push(group(11, "backend", "127.0.0.1:3001"));
        assert!(matches!(
            compile(1, &config(vec![dup_group])),
            Err(CompileError::DuplicateUpstreamName { .. })
        ));

        let mut dup_route = subsystem(1, "shop", &[]);
        dup_route.routes.push(route(2, "/", 10));
        assert!(matches!(
            compile(1, &config(vec![dup_route])),
            Err(CompileError::DuplicateRoutePrefix { .. })
        ));
    }

    #[test]
    fn rejects_a_route_pointing_at_another_subsystems_upstream() {
        let mut shop = subsystem(1, "shop", &[]);
        let blog = subsystem(2, "blog", &[]);
        shop.routes[0].upstream_group_id = blog.upstream_groups[0].id;
        let err = compile(1, &config(vec![shop, blog])).unwrap_err();
        assert!(matches!(err, CompileError::UnknownUpstream { .. }));
    }

    #[test]
    fn rejects_a_route_with_a_missing_upstream_even_when_disabled() {
        let mut shop = subsystem(1, "shop", &[]);
        shop.routes[0].upstream_group_id = UpstreamGroupId(999);
        shop.routes[0].enabled = false;
        assert!(matches!(
            compile(1, &config(vec![shop])),
            Err(CompileError::UnknownUpstream { .. })
        ));
    }

    fn endpoint(id: i64, addr: &str) -> EndpointConfig {
        EndpointConfig {
            id: EndpointId(id),
            addr: EndpointAddr::parse(addr).unwrap(),
            weight: 1,
            state: EndpointState::Enabled,
        }
    }

    #[test]
    fn a_group_needs_one_to_64_endpoints() {
        let mut none = subsystem(1, "shop", &[]);
        none.upstream_groups[0].endpoints.clear();
        assert!(matches!(
            compile(1, &config(vec![none])),
            Err(CompileError::EndpointCount { count: 0, .. })
        ));

        let mut many = subsystem(1, "shop", &[]);
        many.upstream_groups[0].endpoints =
            (0..65).map(|i| endpoint(i, &format!("h{i}:1"))).collect();
        assert!(matches!(
            compile(1, &config(vec![many.clone()])),
            Err(CompileError::EndpointCount { count: 65, .. })
        ));
        many.upstream_groups[0].endpoints.pop();
        let snapshot = compile(1, &config(vec![many])).unwrap();
        assert_eq!(
            snapshot.subsystems[&SubsystemId(1)].upstreams[&UpstreamGroupId(10)]
                .endpoints
                .len(),
            64
        );
    }

    #[test]
    fn rejects_duplicate_addresses_bad_weights_and_bad_policies() {
        let mut dup = subsystem(1, "shop", &[]);
        dup.upstream_groups[0]
            .endpoints
            .push(endpoint(2, "127.0.0.1:3000"));
        assert!(matches!(
            compile(1, &config(vec![dup])),
            Err(CompileError::DuplicateEndpoint { .. })
        ));

        let mut heavy = subsystem(1, "shop", &[]);
        heavy.upstream_groups[0].endpoints[0].weight = 0;
        assert!(matches!(
            compile(1, &config(vec![heavy])),
            Err(CompileError::InvalidUpstream { .. })
        ));

        let mut bad = subsystem(1, "shop", &[]);
        bad.upstream_groups[0].policy.passive.max_ejection_percent = 0;
        let err = compile(1, &config(vec![bad])).unwrap_err();
        assert!(
            matches!(&err, CompileError::InvalidUpstream { reason, .. } if reason.contains("passive.maxEjectionPercent"))
        );
    }

    #[test]
    fn routes_of_one_group_share_a_compiled_upstream_and_unused_groups_are_kept() {
        let mut shop = subsystem(1, "shop", &["a.example.com"]);
        shop.routes.push(route(2, "/api", 10));
        shop.upstream_groups
            .push(group(11, "spare", "127.0.0.1:3001"));
        let snapshot = compile(1, &config(vec![shop])).unwrap();
        let compiled = &snapshot.subsystems[&SubsystemId(1)];
        let (Lookup::Matched { route: root, .. }, Lookup::Matched { route: api, .. }) = (
            snapshot.lookup("a.example.com", "/x"),
            snapshot.lookup("a.example.com", "/api/x"),
        ) else {
            panic!("both paths should match");
        };
        assert!(Arc::ptr_eq(&root.route.upstream, &api.route.upstream));
        assert_eq!(
            compiled.upstreams.len(),
            2,
            "the unused group is compiled too"
        );
    }

    fn carried(config: &Config, previous: &Snapshot) -> Snapshot {
        super::compile(2, config, &previous.runtime_registry()).unwrap()
    }

    fn runtime_of(
        snapshot: &Snapshot,
        group: i64,
        index: usize,
    ) -> Arc<fast_proxy_balancing::EndpointRuntime> {
        snapshot.subsystems[&SubsystemId(1)].upstreams[&UpstreamGroupId(group)].candidates[index]
            .runtime
            .clone()
    }

    #[test]
    fn runtime_state_survives_unrelated_edits_but_not_an_address_change_or_removal() {
        let mut shop = subsystem(1, "shop", &[]);
        shop.upstream_groups[0]
            .endpoints
            .push(endpoint(99, "127.0.0.1:3999"));
        let first = compile(1, &config(vec![shop.clone()])).unwrap();
        let (kept, other) = (runtime_of(&first, 10, 0), runtime_of(&first, 10, 1));

        let mut unrelated = shop.clone();
        unrelated.routes[0].strip_prefix = true;
        unrelated.description = "changed".into();
        let after_unrelated = carried(&config(vec![unrelated]), &first);
        assert!(Arc::ptr_eq(&kept, &runtime_of(&after_unrelated, 10, 0)));
        assert!(Arc::ptr_eq(&other, &runtime_of(&after_unrelated, 10, 1)));

        let mut moved = shop.clone();
        moved.upstream_groups[0].endpoints[1].addr = EndpointAddr::parse("127.0.0.1:4000").unwrap();
        let after_move = carried(&config(vec![moved]), &first);
        assert!(Arc::ptr_eq(&kept, &runtime_of(&after_move, 10, 0)));
        assert!(
            !Arc::ptr_eq(&other, &runtime_of(&after_move, 10, 1)),
            "a new address starts fresh"
        );

        let mut removed = shop.clone();
        removed.upstream_groups[0].endpoints.pop();
        let after_removal = carried(&config(vec![removed]), &first);
        assert_eq!(after_removal.runtime_registry().len(), 1);

        let readded = carried(&config(vec![shop]), &after_removal);
        assert!(
            !Arc::ptr_eq(&other, &runtime_of(&readded, 10, 1)),
            "a removed endpoint is forgotten"
        );
    }

    #[test]
    fn a_failed_compile_leaves_the_previous_registry_usable() {
        let shop = subsystem(1, "shop", &[]);
        let first = compile(1, &config(vec![shop.clone()])).unwrap();
        let registry = first.runtime_registry();
        let mut broken = shop;
        broken.upstream_groups[0].endpoints.clear();
        assert!(super::compile(2, &config(vec![broken]), &registry).is_err());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn disabled_endpoints_compile_as_not_admin_enabled() {
        let mut shop = subsystem(1, "shop", &[]);
        shop.upstream_groups[0].endpoints[0].state = EndpointState::Draining;
        let snapshot = compile(1, &config(vec![shop])).unwrap();
        let upstream = &snapshot.subsystems[&SubsystemId(1)].upstreams[&UpstreamGroupId(10)];
        assert!(!upstream.candidates[0].admin_enabled);
        assert_eq!(upstream.endpoints[0].state, EndpointState::Draining);
    }
}
