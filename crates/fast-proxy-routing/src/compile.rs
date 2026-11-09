use std::collections::{HashMap, HashSet};

use fast_proxy_model::{
    Config, RoutePrefix, SubsystemConfig, UpstreamGroupConfig, UpstreamGroupId,
};

use crate::snapshot::{CompiledRoute, CompiledSubsystem};
use crate::{CompileError, HostTable, RouteTable, Snapshot};

/// Compiles `config` into a snapshot labelled `version`. Pure and
/// deterministic; fails on any violated cross-table rule.
pub fn compile(version: u64, config: &Config) -> Result<Snapshot, CompileError> {
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
        subsystems.insert(subsystem.id, compile_subsystem(subsystem)?);
    }

    Ok(Snapshot {
        version,
        hosts: HostTable::build(host_entries),
        subsystems,
    })
}

fn compile_subsystem(subsystem: &SubsystemConfig) -> Result<CompiledSubsystem, CompileError> {
    let name = subsystem.name.as_str();
    let groups = index_groups(subsystem)?;

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
                strip_prefix: route.strip_prefix,
                upstream_prefix: route.upstream_prefix.clone(),
                preserve_host: route.preserve_host,
                connect_timeout: route.connect_timeout,
                response_timeout: route.response_timeout,
                upstream_name: group.name.clone(),
                endpoint: group.endpoints[0].addr.clone(),
            },
        ));
    }

    Ok(CompiledSubsystem {
        name: subsystem.name.clone(),
        enabled: subsystem.enabled,
        routes: RouteTable::build(routes),
    })
}

/// Indexes a subsystem's upstream groups by id, enforcing unique names and
/// exactly one endpoint per group (the M1 rule).
fn index_groups(
    subsystem: &SubsystemConfig,
) -> Result<HashMap<UpstreamGroupId, &UpstreamGroupConfig>, CompileError> {
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
        if group.endpoints.len() != 1 {
            return Err(CompileError::EndpointCount {
                subsystem: name.to_string(),
                upstream: group.name.clone(),
                count: group.endpoints.len(),
            });
        }
        by_id.insert(group.id, group);
    }
    Ok(by_id)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fast_proxy_model::{
        EndpointAddr, EndpointConfig, HostPattern, RouteConfig, RouteId, SubsystemId,
        SubsystemName, UpstreamPrefix,
    };

    use super::*;

    pub(crate) fn group(id: i64, name: &str, addr: &str) -> UpstreamGroupConfig {
        UpstreamGroupConfig {
            id: UpstreamGroupId(id),
            name: name.into(),
            endpoints: vec![EndpointConfig {
                addr: EndpointAddr::parse(addr).unwrap(),
                weight: 1,
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
            enabled: true,
        }
    }

    pub(crate) fn subsystem(id: i64, name: &str, hosts: &[&str]) -> SubsystemConfig {
        SubsystemConfig {
            id: SubsystemId(id),
            name: SubsystemName::parse(name).unwrap(),
            enabled: true,
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
        assert_eq!(found.route.endpoint.to_string(), "127.0.0.1:3000");
        assert_eq!(found.route.upstream_name, "backend");
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

    #[test]
    fn requires_exactly_one_endpoint_per_upstream() {
        let mut none = subsystem(1, "shop", &[]);
        none.upstream_groups[0].endpoints.clear();
        assert!(matches!(
            compile(1, &config(vec![none])),
            Err(CompileError::EndpointCount { count: 0, .. })
        ));

        let mut two = subsystem(1, "shop", &[]);
        let extra = two.upstream_groups[0].endpoints[0].clone();
        two.upstream_groups[0].endpoints.push(extra);
        assert!(matches!(
            compile(1, &config(vec![two])),
            Err(CompileError::EndpointCount { count: 2, .. })
        ));
    }
}
