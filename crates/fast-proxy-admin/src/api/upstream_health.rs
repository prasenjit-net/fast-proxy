use fast_proxy_balancing::monotonic_ms;
use fast_proxy_routing::Snapshot;
use serde::Serialize;

use super::effective_state::Effective;

/// An endpoint that is not taking requests, and why.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UnhealthyEndpoint {
    subsystem: String,
    upstream: String,
    endpoint: String,
    reason: &'static str,
}

/// How many endpoints are in rotation, with the ones that are not.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UpstreamHealth {
    total: usize,
    healthy: usize,
    unhealthy: Vec<UnhealthyEndpoint>,
}

/// Summarizes every endpoint of the live snapshot. Healthy means taking new
/// requests right now: enabled, passing health checks and not ejected.
pub fn summarize(snapshot: &Snapshot) -> UpstreamHealth {
    let now = monotonic_ms();
    let mut summary = UpstreamHealth {
        total: 0,
        healthy: 0,
        unhealthy: Vec::new(),
    };
    for subsystem in snapshot.subsystems.values() {
        for upstream in subsystem.upstreams.values() {
            for (endpoint, candidate) in upstream.endpoints.iter().zip(&upstream.candidates) {
                summary.total += 1;
                let reason =
                    Effective::classify(endpoint.state, &candidate.runtime, now).summary_reason();
                match reason {
                    None => summary.healthy += 1,
                    Some(reason) => summary.unhealthy.push(UnhealthyEndpoint {
                        subsystem: subsystem.name.as_str().to_string(),
                        upstream: upstream.name.clone(),
                        endpoint: endpoint.addr.to_string(),
                        reason,
                    }),
                }
            }
        }
    }
    summary.unhealthy.sort_by(|a, b| {
        (&a.subsystem, &a.upstream, &a.endpoint).cmp(&(&b.subsystem, &b.upstream, &b.endpoint))
    });
    summary
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_model::{
        Config, EndpointAddr, EndpointConfig, EndpointId, EndpointState, GroupPolicy, HostPattern,
        SubsystemConfig, SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId,
    };
    use fast_proxy_routing::compile;

    use super::*;

    fn snapshot(states: &[EndpointState]) -> Snapshot {
        let endpoints = states
            .iter()
            .enumerate()
            .map(|(index, state)| EndpointConfig {
                id: EndpointId(index as i64 + 1),
                addr: EndpointAddr::parse(&format!("10.0.0.{}:80", index + 1)).unwrap(),
                weight: 1,
                state: *state,
            })
            .collect();
        let config = Config {
            subsystems: vec![SubsystemConfig {
                id: SubsystemId(1),
                name: SubsystemName::parse("shop").unwrap(),
                enabled: true,
                description: String::new(),
                hosts: vec![HostPattern::parse("shop.example.com").unwrap()],
                upstream_groups: vec![UpstreamGroupConfig {
                    id: UpstreamGroupId(1),
                    name: "backend".into(),
                    policy: GroupPolicy::default(),
                    endpoints,
                }],
                routes: vec![],
            }],
        };
        compile(1, &config, &RuntimeRegistry::new()).unwrap()
    }

    fn reasons(summary: &UpstreamHealth) -> Vec<(&str, &str)> {
        summary
            .unhealthy
            .iter()
            .map(|e| (e.endpoint.as_str(), e.reason))
            .collect()
    }

    #[test]
    fn admin_states_explain_themselves_and_healthy_ones_are_counted() {
        let snapshot = snapshot(&[
            EndpointState::Enabled,
            EndpointState::Draining,
            EndpointState::Disabled,
        ]);
        let summary = summarize(&snapshot);
        assert_eq!((summary.total, summary.healthy), (3, 1));
        assert_eq!(
            reasons(&summary),
            [("10.0.0.2:80", "draining"), ("10.0.0.3:80", "disabled")]
        );
    }

    #[test]
    fn failed_probes_and_ejections_are_told_apart() {
        let snapshot = snapshot(&[EndpointState::Enabled, EndpointState::Enabled]);
        let upstream = snapshot
            .subsystems
            .values()
            .next()
            .unwrap()
            .upstreams
            .values()
            .next()
            .unwrap();
        upstream.candidates[0].runtime.record_probe(false, 1, 1);
        let group = &upstream.candidates;
        let policy = fast_proxy_model::PassiveHealth {
            consecutive_failures: 1,
            max_ejection_percent: 100,
            ..fast_proxy_model::PassiveHealth::default()
        };
        // A long ejection starting now, so it is still in force when summarized.
        let now = monotonic_ms();
        let policy = fast_proxy_model::PassiveHealth {
            base_ejection: Duration::from_secs(60),
            max_ejection: Duration::from_secs(60),
            ..policy
        };
        fast_proxy_balancing::record_outcome(
            group,
            1,
            fast_proxy_balancing::Outcome::Failure,
            &policy,
            now,
        );

        let summary = summarize(&snapshot);
        assert_eq!(summary.healthy, 0);
        assert_eq!(
            reasons(&summary),
            [
                ("10.0.0.1:80", "failed health checks"),
                ("10.0.0.2:80", "ejected after repeated failures")
            ]
        );
    }

    #[test]
    fn an_empty_snapshot_has_nothing_to_report() {
        let summary = summarize(&Snapshot::empty());
        assert_eq!((summary.total, summary.healthy), (0, 0));
        assert!(summary.unhealthy.is_empty());
    }
}
