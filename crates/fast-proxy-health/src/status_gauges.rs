use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::monotonic_ms;
use fast_proxy_routing::{Snapshot, SnapshotHolder};
use fast_proxy_telemetry::{EndpointLabels, Metrics};
use tokio_util::sync::CancellationToken;

/// How often the gauges are refreshed.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// Publishes every endpoint's health and in-flight count as gauges once a
/// second, until `shutdown` is cancelled. Series of endpoints that are gone
/// are deleted on each pass, so a removed endpoint cannot linger.
pub async fn run_status_gauges(
    holder: Arc<SnapshotHolder>,
    metrics: Arc<Metrics>,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(SAMPLE_INTERVAL);
    loop {
        tokio::select! {
            _ = ticker.tick() => sample(&holder.load(), &metrics),
            () = shutdown.cancelled() => return,
        }
    }
}

fn sample(snapshot: &Snapshot, metrics: &Metrics) {
    let now = monotonic_ms();
    let mut live = std::collections::HashSet::new();
    for subsystem in snapshot.subsystems.values() {
        for upstream in subsystem.upstreams.values() {
            for (endpoint, candidate) in upstream.endpoints.iter().zip(&upstream.candidates) {
                let addr = endpoint.addr.to_string();
                let labels = EndpointLabels {
                    subsystem: subsystem.name.as_str(),
                    upstream: &upstream.name,
                    endpoint: &addr,
                };
                let runtime = &candidate.runtime;
                let healthy = runtime.is_active_healthy() && !runtime.is_ejected(now);
                metrics.set_endpoint_state(labels, healthy, i64::from(runtime.in_flight()));
                live.insert((
                    subsystem.name.as_str().to_string(),
                    upstream.name.clone(),
                    addr,
                ));
            }
        }
    }
    metrics.retain_upstream_series(&live);
}
