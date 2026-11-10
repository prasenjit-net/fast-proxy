use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_balancing::EndpointRuntime;
use fast_proxy_model::{EndpointId, HealthKind};
use fast_proxy_routing::Snapshot;
use tokio_util::sync::CancellationToken;

use crate::probe::ProbeTls;
use crate::prober::{run_prober, EndpointName, ProbeContext, ProberSpec};

type ProberKey = (EndpointId, String);

struct Running {
    spec: ProberSpec,
    runtime: Arc<EndpointRuntime>,
    cancel: CancellationToken,
}

/// Keeps the set of prober tasks equal to what the live snapshot asks for:
/// one per endpoint of each group with checks enabled. Started again when an
/// endpoint's check settings (or runtime) change, stopped when it goes away.
pub(crate) struct Reconciler {
    context: Arc<ProbeContext>,
    running: HashMap<ProberKey, Running>,
}

impl Reconciler {
    pub(crate) fn new(context: Arc<ProbeContext>) -> Self {
        Self {
            context,
            running: HashMap::new(),
        }
    }

    /// Brings the running probers in line with `snapshot`.
    pub(crate) fn reconcile(&mut self, snapshot: &Snapshot) {
        let wanted = desired(snapshot);
        self.running.retain(|key, running| {
            let keep = wanted.get(key).is_some_and(|(spec, runtime)| {
                *spec == running.spec && Arc::ptr_eq(runtime, &running.runtime)
            });
            if !keep {
                running.cancel.cancel();
            }
            keep
        });
        for (key, (spec, runtime)) in wanted {
            if self.running.contains_key(&key) {
                continue;
            }
            let cancel = CancellationToken::new();
            tokio::spawn(run_prober(
                spec.clone(),
                runtime.clone(),
                self.context.clone(),
                cancel.clone(),
            ));
            self.running.insert(
                key,
                Running {
                    spec,
                    runtime,
                    cancel,
                },
            );
        }
    }

    /// `upstream/endpoint` of every running prober, sorted.
    #[cfg(test)]
    pub(crate) fn endpoints(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .running
            .values()
            .map(|running| {
                format!(
                    "{}/{}",
                    running.spec.name.upstream, running.spec.name.endpoint
                )
            })
            .collect();
        names.sort();
        names
    }

    /// Cancellation tokens of the running probers, to observe restarts.
    #[cfg(test)]
    pub(crate) fn tokens(&self) -> Vec<CancellationToken> {
        self.running.values().map(|r| r.cancel.clone()).collect()
    }

    /// Stops every prober.
    pub(crate) fn stop_all(&mut self) {
        for running in self.running.values() {
            running.cancel.cancel();
        }
        self.running.clear();
    }
}

/// How to probe an endpoint of a group with upstream TLS on: the group's
/// client configuration and the name to verify (the group's override, else the
/// endpoint's host). `None` for a plain group or an unusable name.
fn probe_tls(
    upstream: &fast_proxy_routing::CompiledUpstream,
    addr: &fast_proxy_model::EndpointAddr,
) -> Option<ProbeTls> {
    let config = upstream.client_tls.clone()?;
    let server_name =
        fast_proxy_tls::server_name_for(&upstream.policy.tls.server_name, addr.host()).ok()?;
    Some(ProbeTls {
        config,
        server_name,
    })
}

/// The probers the snapshot calls for, keyed by endpoint id and address.
fn desired(snapshot: &Snapshot) -> HashMap<ProberKey, (ProberSpec, Arc<EndpointRuntime>)> {
    let mut wanted = HashMap::new();
    for subsystem in snapshot.subsystems.values() {
        for upstream in subsystem.upstreams.values() {
            if upstream.policy.health.kind == HealthKind::None {
                continue;
            }
            for (endpoint, candidate) in upstream.endpoints.iter().zip(&upstream.candidates) {
                let spec = ProberSpec {
                    name: EndpointName {
                        subsystem: subsystem.name.as_str().to_string(),
                        upstream: upstream.name.clone(),
                        endpoint: endpoint.addr.to_string(),
                    },
                    addr: endpoint.addr.clone(),
                    check: upstream.policy.health.clone(),
                    tls: probe_tls(upstream, &endpoint.addr),
                };
                wanted.insert(
                    (endpoint.id, endpoint.addr.to_string()),
                    (spec, candidate.runtime.clone()),
                );
            }
        }
    }
    wanted
}

#[cfg(test)]
mod tls_tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;

    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_routing::{compile_with_tls, TlsInputs};
    use fast_proxy_test_pki::TestCa;
    use fast_proxy_tls::CaBundle;

    use super::*;
    use crate::test_support::{config_for, fast_check};

    const ADDR: &str = "127.0.0.1:9443";

    fn desired_for(tls: Option<(&str, &str, i64)>, inputs: &TlsInputs) -> ProberSpec {
        let mut config = config_for(
            fast_check("/healthz"),
            &[ADDR.parse::<SocketAddr>().unwrap()],
        );
        let group = &mut config.subsystems[0].upstream_groups[0];
        if let Some((name, _, _)) = tls {
            group.policy.tls.enabled = true;
            group.policy.tls.ca = name.into();
            group.policy.tls.server_name = "backend.internal".into();
        }
        let snapshot = compile_with_tls(1, &config, &RuntimeRegistry::new(), inputs).unwrap();
        desired(&snapshot).into_values().next().unwrap().0
    }

    fn inputs(name: &str, pem: &str, updated: i64) -> TlsInputs {
        TlsInputs {
            ca_bundles: HashMap::from([(
                name.to_string(),
                CaBundle {
                    name: name.into(),
                    pem: pem.into(),
                    updated_at_ms: updated,
                },
            )]),
            ..TlsInputs::off()
        }
    }

    #[test]
    fn a_tls_group_is_probed_over_tls_with_its_name_and_a_plain_group_is_not() {
        let ca = TestCa::new("corp root");
        let tls_inputs = inputs("corp", &ca.pem, 1);
        let secure = desired_for(Some(("corp", &ca.pem, 1)), &tls_inputs);
        let probe = secure.tls.expect("a TLS group is probed over TLS");
        assert_eq!(
            probe.server_name,
            fast_proxy_tls::server_name_for("backend.internal", "127.0.0.1").unwrap(),
            "the group's override name wins over the endpoint host"
        );
        assert!(desired_for(None, &TlsInputs::off()).tls.is_none());
    }

    #[test]
    fn replacing_the_bundle_changes_the_spec_so_the_prober_restarts() {
        let (old, new) = (TestCa::new("old"), TestCa::new("new"));
        let shared = TlsInputs::off().client_configs;
        let before = TlsInputs {
            client_configs: shared.clone(),
            ..inputs("corp", &old.pem, 1)
        };
        let after = TlsInputs {
            client_configs: shared,
            ..inputs("corp", &new.pem, 2)
        };
        let one = desired_for(Some(("corp", &old.pem, 1)), &before);
        let same = desired_for(Some(("corp", &old.pem, 1)), &before);
        let two = desired_for(Some(("corp", &new.pem, 2)), &after);
        assert_eq!(one, same, "an unchanged bundle keeps the running prober");
        assert_ne!(one, two, "a replaced bundle restarts it");
    }
}
