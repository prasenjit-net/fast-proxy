use std::sync::Arc;

use fast_proxy_config_service::ConfigChanged;
use fast_proxy_routing::SnapshotHolder;
use fast_proxy_telemetry::{EndpointEvents, Metrics};
use tokio::sync::{broadcast, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::prober::ProbeContext;
use crate::reconciler::Reconciler;

/// Most probes allowed in flight at once, however many endpoints there are.
const MAX_CONCURRENT_PROBES: usize = 64;

/// Runs the active health checks for the live configuration.
pub struct HealthEngine {
    holder: Arc<SnapshotHolder>,
    context: Arc<ProbeContext>,
}

impl HealthEngine {
    pub fn new(holder: Arc<SnapshotHolder>, metrics: Arc<Metrics>, events: EndpointEvents) -> Self {
        Self {
            holder,
            context: Arc::new(ProbeContext {
                metrics,
                events,
                concurrency: Semaphore::new(MAX_CONCURRENT_PROBES),
            }),
        }
    }

    /// Starts probers for the current snapshot, then follows every
    /// configuration change until `shutdown` is cancelled, when all probers
    /// stop.
    pub async fn run(
        self,
        mut changes: broadcast::Receiver<ConfigChanged>,
        shutdown: CancellationToken,
    ) {
        let mut reconciler = Reconciler::new(self.context.clone());
        reconciler.reconcile(&self.holder.load());
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                change = changes.recv() => match change {
                    // Lagging means changes were missed; the latest snapshot
                    // covers them all.
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                        reconciler.reconcile(&self.holder.load());
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
        reconciler.stop_all();
    }
}
