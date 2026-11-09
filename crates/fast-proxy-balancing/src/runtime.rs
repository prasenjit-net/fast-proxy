use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;

/// Time constant of the latency average: older samples fade over about this long.
const EWMA_TAU_MS: f64 = 10_000.0;

/// An endpoint's live state: in-flight requests, health and latency. Every
/// field is an atomic, so the request path never takes a lock. One instance
/// is shared by every snapshot that contains the endpoint.
#[derive(Debug)]
pub struct EndpointRuntime {
    in_flight: AtomicU32,
    active_healthy: AtomicBool,
    /// Consecutive probes that contradict the current active state.
    probe_streak: AtomicU32,
    pub(crate) consecutive_failures: AtomicU32,
    /// 0 when not ejected; otherwise the time the ejection ends.
    pub(crate) ejected_until_ms: AtomicU64,
    pub(crate) ejection_count: AtomicU32,
    /// A half-open trial request is in flight.
    pub(crate) trial_in_flight: AtomicBool,
    ewma_us: AtomicU64,
    ewma_at_ms: AtomicU64,
}

/// A point-in-time copy of an endpoint's state, for status pages and metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub in_flight: u32,
    pub active_healthy: bool,
    /// The end of the current ejection, if the endpoint is ejected.
    pub ejected_until_ms: Option<u64>,
    pub ewma_us: u64,
}

impl Default for EndpointRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl EndpointRuntime {
    /// A new endpoint starts healthy (optimistic) with no samples.
    pub fn new() -> Self {
        Self {
            in_flight: AtomicU32::new(0),
            active_healthy: AtomicBool::new(true),
            probe_streak: AtomicU32::new(0),
            consecutive_failures: AtomicU32::new(0),
            ejected_until_ms: AtomicU64::new(0),
            ejection_count: AtomicU32::new(0),
            trial_in_flight: AtomicBool::new(false),
            ewma_us: AtomicU64::new(0),
            ewma_at_ms: AtomicU64::new(0),
        }
    }

    /// Counts a request as in flight until the returned guard is dropped.
    pub fn track(self: &Arc<Self>) -> InFlightGuard {
        self.in_flight.fetch_add(1, Relaxed);
        InFlightGuard {
            runtime: Arc::clone(self),
        }
    }

    /// Requests currently in flight on this endpoint.
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(Relaxed)
    }

    /// Whether active probes (if any) consider the endpoint healthy.
    pub fn is_active_healthy(&self) -> bool {
        self.active_healthy.load(Relaxed)
    }

    /// Whether the endpoint is inside an ejection at `now_ms`.
    pub fn is_ejected(&self, now_ms: u64) -> bool {
        let until = self.ejected_until_ms.load(Relaxed);
        until != 0 && now_ms < until
    }

    /// Whether health allows new requests at `now_ms`: active-healthy, and
    /// either never ejected, or ejected with the ejection over and no trial
    /// request already out (half-open).
    pub fn is_routable(&self, now_ms: u64) -> bool {
        if !self.is_active_healthy() {
            return false;
        }
        let until = self.ejected_until_ms.load(Relaxed);
        until == 0 || (now_ms >= until && !self.trial_in_flight.load(Relaxed))
    }

    /// Called when this endpoint was chosen for a request. Marks the trial
    /// request of a half-open endpoint so others wait for its verdict. Two
    /// racing selections may both become trials; that is tolerated.
    pub fn mark_selected(&self, now_ms: u64) {
        let until = self.ejected_until_ms.load(Relaxed);
        if until != 0 && now_ms >= until {
            self.trial_in_flight.store(true, Relaxed);
        }
    }

    /// Feeds one active probe result through the rise/fall hysteresis.
    /// Returns the new health when this probe flipped it.
    pub fn record_probe(&self, ok: bool, rise: u32, fall: u32) -> Option<bool> {
        if ok == self.is_active_healthy() {
            self.probe_streak.store(0, Relaxed);
            return None;
        }
        let streak = self.probe_streak.fetch_add(1, Relaxed) + 1;
        if streak < if ok { rise } else { fall } {
            return None;
        }
        self.probe_streak.store(0, Relaxed);
        self.active_healthy.store(ok, Relaxed);
        Some(ok)
    }

    /// Folds a latency sample (time to first byte) into the peak-EWMA: a
    /// slower sample replaces the average at once, a faster one pulls it down
    /// as older samples fade with time.
    pub fn record_latency(&self, sample_us: u64, now_ms: u64) {
        let current = self.ewma_us.load(Relaxed);
        let next = if current == 0 || sample_us > current {
            sample_us
        } else {
            let elapsed = now_ms.saturating_sub(self.ewma_at_ms.load(Relaxed)) as f64;
            let keep = (-elapsed / EWMA_TAU_MS).exp();
            (current as f64 * keep + sample_us as f64 * (1.0 - keep)) as u64
        };
        self.ewma_us.store(next.max(1), Relaxed);
        self.ewma_at_ms.store(now_ms, Relaxed);
    }

    /// The latency average in microseconds; 0 until the first sample.
    pub fn ewma_us(&self) -> u64 {
        self.ewma_us.load(Relaxed)
    }

    /// A copy of the state as of `now_ms`.
    pub fn status(&self, now_ms: u64) -> RuntimeStatus {
        RuntimeStatus {
            in_flight: self.in_flight(),
            active_healthy: self.is_active_healthy(),
            ejected_until_ms: Some(self.ejected_until_ms.load(Relaxed))
                .filter(|until| *until != 0 && now_ms < *until),
            ewma_us: self.ewma_us(),
        }
    }
}

/// Holds an endpoint's in-flight count up while a request (including its
/// streamed response) is alive.
#[derive(Debug)]
pub struct InFlightGuard {
    runtime: Arc<EndpointRuntime>,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.runtime.in_flight.fetch_sub(1, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_endpoint_is_healthy_routable_and_idle() {
        let runtime = EndpointRuntime::new();
        assert!(runtime.is_routable(0) && !runtime.is_ejected(0));
        assert_eq!(runtime.in_flight(), 0);
        assert_eq!(runtime.ewma_us(), 0);
    }

    #[test]
    fn the_guard_counts_in_flight_requests_and_releases_on_drop() {
        let runtime = Arc::new(EndpointRuntime::new());
        let first = runtime.track();
        let second = runtime.track();
        assert_eq!(runtime.in_flight(), 2);
        drop(first);
        assert_eq!(runtime.in_flight(), 1);
        drop(second);
        assert_eq!(runtime.in_flight(), 0);
    }

    #[test]
    fn probes_need_fall_failures_then_rise_successes_to_flip() {
        let runtime = EndpointRuntime::new();
        assert_eq!(runtime.record_probe(false, 2, 3), None);
        assert_eq!(runtime.record_probe(false, 2, 3), None);
        assert_eq!(
            runtime.record_probe(true, 2, 3),
            None,
            "a good probe resets the streak"
        );
        assert_eq!(runtime.record_probe(false, 2, 3), None);
        assert_eq!(runtime.record_probe(false, 2, 3), None);
        assert_eq!(runtime.record_probe(false, 2, 3), Some(false));
        assert!(!runtime.is_routable(0));
        assert_eq!(runtime.record_probe(true, 2, 3), None);
        assert_eq!(runtime.record_probe(true, 2, 3), Some(true));
        assert!(runtime.is_routable(0));
    }

    #[test]
    fn peak_ewma_jumps_up_and_decays_down_with_time() {
        let runtime = EndpointRuntime::new();
        runtime.record_latency(1_000, 0);
        assert_eq!(runtime.ewma_us(), 1_000);
        runtime.record_latency(5_000, 1);
        assert_eq!(runtime.ewma_us(), 5_000, "slower samples win at once");
        runtime.record_latency(1_000, 1);
        assert!(runtime.ewma_us() > 4_900, "no time passed, almost no decay");
        runtime.record_latency(1_000, 100_000);
        assert!(
            runtime.ewma_us() < 1_100,
            "long after, the new sample dominates"
        );
    }

    #[test]
    fn a_half_open_endpoint_is_routable_until_its_trial_is_marked() {
        let runtime = EndpointRuntime::new();
        runtime.ejected_until_ms.store(100, Relaxed);
        assert!(runtime.is_ejected(99) && !runtime.is_routable(99));
        assert!(runtime.is_routable(100));
        runtime.mark_selected(100);
        assert!(!runtime.is_routable(100), "one trial at a time");
        assert_eq!(runtime.status(50).ejected_until_ms, Some(100));
        assert_eq!(runtime.status(100).ejected_until_ms, None);
    }

    #[test]
    fn marking_a_healthy_endpoint_changes_nothing() {
        let runtime = EndpointRuntime::new();
        runtime.mark_selected(5);
        assert!(runtime.is_routable(5));
    }
}
