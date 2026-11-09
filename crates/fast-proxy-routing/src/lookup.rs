use fast_proxy_model::SubsystemId;

use crate::{CompiledRoute, CompiledSubsystem, RouteMatch, Snapshot};

/// The outcome of routing one request against a snapshot.
#[derive(Debug)]
pub enum Lookup<'snapshot, 'path> {
    /// No subsystem answers to this `Host`.
    UnknownHost,
    /// The subsystem exists but is switched off.
    SubsystemDisabled {
        id: SubsystemId,
        subsystem: &'snapshot CompiledSubsystem,
    },
    /// The subsystem has no route matching the path.
    NoRoute {
        id: SubsystemId,
        subsystem: &'snapshot CompiledSubsystem,
    },
    Matched {
        id: SubsystemId,
        subsystem: &'snapshot CompiledSubsystem,
        route: RouteMatch<'snapshot, 'path, CompiledRoute>,
    },
}

impl Snapshot {
    /// Routes a request: host to subsystem, then path to route.
    /// `path` excludes the query string.
    pub fn lookup<'snapshot, 'path>(
        &'snapshot self,
        host_header: &str,
        path: &'path str,
    ) -> Lookup<'snapshot, 'path> {
        let Some(id) = self.hosts.lookup(host_header) else {
            return Lookup::UnknownHost;
        };
        let Some(subsystem) = self.subsystems.get(&id) else {
            return Lookup::UnknownHost;
        };
        if !subsystem.enabled {
            return Lookup::SubsystemDisabled { id, subsystem };
        }
        match subsystem.routes.find(path) {
            Some(route) => Lookup::Matched {
                id,
                subsystem,
                route,
            },
            None => Lookup::NoRoute { id, subsystem },
        }
    }
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{
        Config, EndpointAddr, EndpointConfig, HostPattern, RouteConfig, RouteId, RoutePrefix,
        SubsystemConfig, SubsystemName, UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix,
    };

    use super::*;
    use crate::compile;

    fn subsystem(
        id: i64,
        name: &str,
        host: &str,
        addr: &str,
        prefixes: &[&str],
    ) -> SubsystemConfig {
        SubsystemConfig {
            id: SubsystemId(id),
            name: SubsystemName::parse(name).unwrap(),
            enabled: true,
            hosts: vec![HostPattern::parse(host).unwrap()],
            upstream_groups: vec![UpstreamGroupConfig {
                id: UpstreamGroupId(id),
                name: "backend".into(),
                endpoints: vec![EndpointConfig {
                    addr: EndpointAddr::parse(addr).unwrap(),
                    weight: 1,
                }],
            }],
            routes: prefixes
                .iter()
                .enumerate()
                .map(|(index, prefix)| RouteConfig {
                    id: RouteId(id * 100 + index as i64),
                    prefix: RoutePrefix::parse(prefix).unwrap(),
                    upstream_group_id: UpstreamGroupId(id),
                    strip_prefix: false,
                    upstream_prefix: UpstreamPrefix::default(),
                    preserve_host: true,
                    connect_timeout: std::time::Duration::from_secs(5),
                    response_timeout: std::time::Duration::from_secs(30),
                    enabled: true,
                })
                .collect(),
        }
    }

    fn snapshot() -> Snapshot {
        let mut off = subsystem(3, "off", "off.example.com", "127.0.0.1:3002", &["/"]);
        off.enabled = false;
        let config = Config {
            subsystems: vec![
                subsystem(
                    1,
                    "shop",
                    "shop.example.com",
                    "127.0.0.1:3000",
                    &["/", "/api"],
                ),
                subsystem(2, "blog", "blog.example.com", "127.0.0.1:3001", &["/posts"]),
                off,
            ],
        };
        compile(1, &config).unwrap()
    }

    #[test]
    fn two_hosts_route_to_different_backends() {
        let snapshot = snapshot();
        let Lookup::Matched { route, .. } = snapshot.lookup("shop.example.com", "/x") else {
            panic!("expected a match");
        };
        assert_eq!(route.route.endpoint.to_string(), "127.0.0.1:3000");
        let Lookup::Matched { route, .. } = snapshot.lookup("blog.example.com:8000", "/posts/1")
        else {
            panic!("expected a match");
        };
        assert_eq!(route.route.endpoint.to_string(), "127.0.0.1:3001");
        assert_eq!(route.remainder, "/1");
    }

    #[test]
    fn the_longest_prefix_inside_a_subsystem_wins() {
        let snapshot = snapshot();
        let Lookup::Matched { route, .. } = snapshot.lookup("shop.example.com", "/api/users")
        else {
            panic!("expected a match");
        };
        assert_eq!(route.route.id, RouteId(101));
    }

    #[test]
    fn reports_each_failure_kind() {
        let snapshot = snapshot();
        assert!(matches!(
            snapshot.lookup("nope.example.com", "/"),
            Lookup::UnknownHost
        ));
        assert!(matches!(snapshot.lookup("", "/"), Lookup::UnknownHost));
        assert!(matches!(
            snapshot.lookup("off.example.com", "/"),
            Lookup::SubsystemDisabled {
                id: SubsystemId(3),
                ..
            }
        ));
        assert!(matches!(
            snapshot.lookup("blog.example.com", "/other"),
            Lookup::NoRoute {
                id: SubsystemId(2),
                ..
            }
        ));
    }

    #[test]
    fn an_empty_snapshot_knows_no_hosts() {
        assert!(matches!(
            Snapshot::empty().lookup("a.com", "/"),
            Lookup::UnknownHost
        ));
    }
}
