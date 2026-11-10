use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_balancing::EndpointRuntime;
use fast_proxy_model::{EndpointId, HealthKind};
use fast_proxy_routing::Snapshot;
use tokio_util::sync::CancellationToken;

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
