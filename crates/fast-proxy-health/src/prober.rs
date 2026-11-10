use fast_proxy_balancing::{monotonic_ms, EndpointRuntime};
use fast_proxy_model::{EndpointAddr, HealthCheck};
use fast_proxy_telemetry::{
    EndpointChange, EndpointEvent, EndpointEvents, EndpointLabels, Metrics,
};
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::probe::{run_probe, ProbeSpec, ProbeTls};

/// Names an endpoint in metrics and events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndpointName {
    pub subsystem: String,
    pub upstream: String,
    pub endpoint: String,
}

impl EndpointName {
    fn labels(&self) -> EndpointLabels<'_> {
        EndpointLabels {
            subsystem: &self.subsystem,
            upstream: &self.upstream,
            endpoint: &self.endpoint,
        }
    }
}

/// Everything one prober task needs. Equal specs mean an existing prober can
/// keep running; any difference restarts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProberSpec {
    pub name: EndpointName,
    pub addr: EndpointAddr,
    pub check: HealthCheck,
    /// Set for an endpoint of a TLS group; the HTTP probe then handshakes.
    pub tls: Option<ProbeTls>,
}

/// What every prober shares.
pub(crate) struct ProbeContext {
    pub metrics: Arc<Metrics>,
    pub events: EndpointEvents,
    /// Bounds how many probes run at once across all endpoints.
    pub concurrency: Semaphore,
}

/// Probes one endpoint every interval until cancelled. The first probe is
/// delayed by a random share of the interval so probers do not fire in step.
pub(crate) async fn run_prober(
    spec: ProberSpec,
    runtime: Arc<EndpointRuntime>,
    context: Arc<ProbeContext>,
    cancel: CancellationToken,
) {
    let interval = spec.check.interval;
    let jitter = interval.mul_f64(rand::random::<f64>());
    tokio::select! {
        () = tokio::time::sleep(jitter) => {}
        () = cancel.cancelled() => return,
    }
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let probe = ProbeSpec::from_check(&spec.check);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            () = cancel.cancelled() => return,
        }
        let Ok(_permit) = context.concurrency.acquire().await else {
            return;
        };
        let result = tokio::select! {
            result = run_probe(&probe, &spec.addr, spec.tls.as_ref()) => result,
            () = cancel.cancelled() => return,
        };
        record(&spec, &runtime, &context, result);
    }
}

/// Counts the probe and applies it to the endpoint's rise/fall state.
fn record(
    spec: &ProberSpec,
    runtime: &EndpointRuntime,
    context: &ProbeContext,
    result: Result<(), String>,
) {
    let ok = result.is_ok();
    runtime.note_probe(ok, monotonic_ms());
    context.metrics.health_check(spec.name.labels(), ok);
    let Some(healthy) = runtime.record_probe(ok, spec.check.rise, spec.check.fall) else {
        return;
    };
    let change = if healthy {
        tracing::info!(upstream = %spec.name.upstream, endpoint = %spec.name.endpoint, "endpoint is healthy");
        EndpointChange::Healthy
    } else {
        let reason = result.err().unwrap_or_default();
        tracing::warn!(upstream = %spec.name.upstream, endpoint = %spec.name.endpoint, %reason, "endpoint is unhealthy");
        EndpointChange::Unhealthy { reason }
    };
    context.events.publish(EndpointEvent {
        subsystem: spec.name.subsystem.clone(),
        upstream: spec.name.upstream.clone(),
        endpoint: spec.name.endpoint.clone(),
        change,
    });
}
