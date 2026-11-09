use std::sync::atomic::Ordering;
use std::time::Duration;

use rand::Rng;
use serde::Serialize;

use crate::services::events::Event;
use crate::state::SharedState;

/// A point-in-time snapshot of server metrics.
///
/// CPU and memory are a random walk so the dashboard has live data out
/// of the box; the request/connection counters are real. Swap the walk
/// for actual sampling (e.g. the `sysinfo` crate) when you need truth.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub cpu: f64,
    pub memory: f64,
    pub requests_total: u64,
    pub requests_per_min: f64,
    pub ws_clients: usize,
    pub uptime_secs: u64,
    pub timestamp_ms: i64,
}

const TICK: Duration = Duration::from_secs(2);

/// Background sampler: stores the latest snapshot (for `GET /api/metrics`)
/// and pushes it to every WebSocket subscriber each tick.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        let mut cpu = 34.0_f64;
        let mut memory = 52.0_f64;
        let mut last_total = 0_u64;

        loop {
            interval.tick().await;

            let (cpu_step, mem_step) = {
                let mut rng = rand::thread_rng();
                (rng.gen_range(-4.5..4.5), rng.gen_range(-2.0..2.0))
            };
            cpu = (cpu + cpu_step).clamp(3.0, 96.0);
            memory = (memory + mem_step).clamp(18.0, 90.0);

            let requests_total = state.requests_total.load(Ordering::Relaxed);
            let requests_per_min =
                requests_total.saturating_sub(last_total) as f64 * (60.0 / TICK.as_secs_f64());
            last_total = requests_total;

            let snapshot = MetricsSnapshot {
                cpu,
                memory,
                requests_total,
                requests_per_min,
                ws_clients: state.ws_clients.load(Ordering::Relaxed),
                uptime_secs: state.started.elapsed().as_secs(),
                timestamp_ms: chrono::Utc::now().timestamp_millis(),
            };

            *state.latest_metrics.write().await = Some(snapshot.clone());
            state.broadcast(Event::Metrics { data: snapshot });
        }
    });
}
