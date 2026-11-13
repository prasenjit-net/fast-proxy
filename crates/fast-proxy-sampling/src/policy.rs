//! The sampling policy of one route: the global settings with the subsystem's
//! and the route's overrides already applied. Immutable; the data plane reads
//! it from the snapshot it loaded for the request.

use fast_proxy_model::{effective, SamplingOverride, SamplingSettings, TriggerSet};

/// What the decision needs to know about a route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingPolicy {
    pub enabled: bool,
    pub capture_bodies: bool,
    pub triggers: TriggerSet,
    pub baseline_per_mille: u32,
    pub slow_threshold_ms: u32,
    pub slow_p99_factor_x10: u32,
    pub slow_p99_floor_ms: u32,
    pub client_errors_per_min: u32,
    pub oversized_request_bytes: u64,
    pub oversized_response_bytes: u64,
    pub rate_per_sec: u32,
    pub rate_burst: u32,
    pub storm_burst: u32,
    pub storm_interval_secs: u32,
    pub body_request_cap: u32,
    pub body_response_cap: u32,
}

impl SamplingPolicy {
    /// Resolves the policy for one route from the global settings and the
    /// overrides of its subsystem and of the route itself.
    pub fn resolve(
        global: &SamplingSettings,
        subsystem: Option<&SamplingOverride>,
        route: Option<&SamplingOverride>,
    ) -> Self {
        let resolved = effective(global, subsystem, route);
        Self {
            enabled: resolved.enabled,
            capture_bodies: resolved.capture_bodies,
            triggers: global.triggers,
            baseline_per_mille: resolved.baseline_per_mille,
            slow_threshold_ms: resolved.slow_threshold_ms,
            slow_p99_factor_x10: resolved.slow_p99_factor_x10,
            slow_p99_floor_ms: global.slow_p99_floor_ms,
            client_errors_per_min: global.client_errors_per_min,
            oversized_request_bytes: global.oversized_request_bytes,
            oversized_response_bytes: global.oversized_response_bytes,
            rate_per_sec: global.rate_per_sec,
            rate_burst: global.rate_burst,
            storm_burst: global.storm_burst,
            storm_interval_secs: global.storm_interval_secs,
            body_request_cap: global.body_request_cap,
            body_response_cap: global.body_response_cap,
        }
    }

    /// A policy that samples nothing.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::resolve(&SamplingSettings::default(), None, None)
        }
    }
}

impl Default for SamplingPolicy {
    /// Samples nothing: the policy of an empty snapshot.
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_override(enabled: Option<bool>, baseline: Option<u32>) -> SamplingOverride {
        SamplingOverride {
            enabled,
            capture_bodies: None,
            baseline_per_mille: baseline,
            slow_threshold_ms: None,
            slow_p99_factor_x10: None,
            set_by: "ops".into(),
            set_at_ms: 0,
        }
    }

    #[test]
    fn the_defaults_resolve_to_the_documented_policy() {
        let policy = SamplingPolicy::resolve(&SamplingSettings::default(), None, None);
        assert!(policy.enabled);
        assert!(!policy.capture_bodies);
        assert_eq!(policy.triggers, TriggerSet::defaults());
        assert_eq!(
            (policy.baseline_per_mille, policy.slow_threshold_ms),
            (1, 2000)
        );
        assert_eq!((policy.storm_burst, policy.storm_interval_secs), (3, 30));
    }

    #[test]
    fn overrides_change_only_what_they_set() {
        let global = SamplingSettings::default();
        let route = route_override(Some(false), Some(500));
        let policy = SamplingPolicy::resolve(&global, None, Some(&route));
        assert!(!policy.enabled);
        assert_eq!(policy.baseline_per_mille, 500);
        assert_eq!(policy.slow_threshold_ms, global.slow_threshold_ms);
        assert_eq!(policy.rate_per_sec, global.rate_per_sec);
    }

    #[test]
    fn a_disabled_policy_keeps_the_other_defaults() {
        let policy = SamplingPolicy::disabled();
        assert!(!policy.enabled);
        assert_eq!(policy.slow_threshold_ms, 2000);
        assert_eq!(SamplingPolicy::default(), policy);
    }
}
