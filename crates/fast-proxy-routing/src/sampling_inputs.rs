//! What the compile step takes besides the configuration to know how each route
//! is sampled, and the runtime state that must survive a snapshot swap.
//! Sampling settings are not part of `Config` (a rollback never changes them),
//! so they are passed beside it, like maintenance.

use std::collections::HashMap;
use std::sync::Arc;

use fast_proxy_model::{SamplingOverride, SamplingSettings, SamplingTarget};
use fast_proxy_sampling::{ClientErrors, LatencyWindow, RateLimiter, StormGuard};

/// The state that belongs to the process or to a route and must not be reset by
/// a configuration change: the per-client 4xx table, the storm guard, the rate
/// limiter, and each route's rolling latency (carried by subsystem name and
/// route prefix, like endpoint state is carried by endpoint id).
#[derive(Debug, Clone)]
pub struct SamplingRuntime {
    pub clients: Arc<ClientErrors>,
    pub storm_guard: Arc<StormGuard>,
    pub rate_limiter: Arc<RateLimiter>,
    windows: Arc<HashMap<(String, String), Arc<LatencyWindow>>>,
}

impl Default for SamplingRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl SamplingRuntime {
    /// A runtime with no history, for a first load.
    pub fn new() -> Self {
        Self {
            clients: Arc::new(ClientErrors::new()),
            storm_guard: Arc::new(StormGuard::new()),
            rate_limiter: Arc::new(RateLimiter::new()),
            windows: Arc::new(HashMap::new()),
        }
    }

    /// The same process-wide state with a latency window for each of `routes`
    /// (subsystem name, route prefix): the existing window where there is one,
    /// a new one otherwise. Windows of routes that are gone are dropped.
    pub fn rebuilt_for(&self, routes: impl IntoIterator<Item = (String, String)>) -> Self {
        let windows = routes
            .into_iter()
            .map(|key| {
                let window = self.windows.get(&key).cloned().unwrap_or_default();
                (key, window)
            })
            .collect();
        Self {
            windows: Arc::new(windows),
            ..self.clone()
        }
    }

    /// The rolling latency of a route, when the route is in this runtime.
    pub fn window(&self, subsystem: &str, prefix: &str) -> Option<Arc<LatencyWindow>> {
        self.windows
            .get(&(subsystem.to_string(), prefix.to_string()))
            .cloned()
    }

    /// Every route window, for the periodic task that publishes the p99s.
    pub fn windows(&self) -> impl Iterator<Item = &Arc<LatencyWindow>> {
        self.windows.values()
    }
}

/// The sampling settings to compile into a snapshot, with the runtime state to
/// carry over.
#[derive(Debug, Clone)]
pub struct SamplingInputs {
    pub settings: SamplingSettings,
    /// Every stored override.
    pub overrides: Vec<(SamplingTarget, SamplingOverride)>,
    /// Usually the live snapshot's [`crate::Snapshot::sampling_runtime`].
    pub runtime: SamplingRuntime,
}

impl SamplingInputs {
    /// The default settings, no overrides and a fresh runtime.
    pub fn none() -> Self {
        Self {
            settings: SamplingSettings::default(),
            overrides: Vec::new(),
            runtime: SamplingRuntime::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(subsystem: &str, prefix: &str) -> (String, String) {
        (subsystem.into(), prefix.into())
    }

    #[test]
    fn a_route_keeps_its_window_across_a_rebuild_and_a_new_route_gets_a_fresh_one() {
        let first = SamplingRuntime::new().rebuilt_for([key("shop", "/api")]);
        let api = first.window("shop", "/api").unwrap();
        api.record(100, 0);
        let second = first.rebuilt_for([key("shop", "/api"), key("shop", "/new")]);
        assert!(Arc::ptr_eq(&api, &second.window("shop", "/api").unwrap()));
        assert!(!Arc::ptr_eq(&api, &second.window("shop", "/new").unwrap()));
    }

    #[test]
    fn a_removed_route_loses_its_window_and_a_recreated_one_starts_fresh() {
        let first = SamplingRuntime::new().rebuilt_for([key("shop", "/api")]);
        let old = first.window("shop", "/api").unwrap();
        let without = first.rebuilt_for([]);
        assert!(without.window("shop", "/api").is_none());
        let again = without.rebuilt_for([key("shop", "/api")]);
        assert!(!Arc::ptr_eq(&old, &again.window("shop", "/api").unwrap()));
    }

    #[test]
    fn the_process_wide_state_is_shared_by_every_rebuild() {
        let first = SamplingRuntime::new();
        let second = first.rebuilt_for([key("a", "/")]);
        assert!(Arc::ptr_eq(&first.clients, &second.clients));
        assert!(Arc::ptr_eq(&first.storm_guard, &second.storm_guard));
        assert!(Arc::ptr_eq(&first.rate_limiter, &second.rate_limiter));
    }

    #[test]
    fn windows_are_keyed_by_subsystem_and_prefix_together() {
        let runtime = SamplingRuntime::new().rebuilt_for([key("a", "/x"), key("b", "/x")]);
        let (a, b) = (
            runtime.window("a", "/x").unwrap(),
            runtime.window("b", "/x").unwrap(),
        );
        assert!(!Arc::ptr_eq(&a, &b));
        assert_eq!(runtime.windows().count(), 2);
    }
}
