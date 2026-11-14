//! A route's sampling as the data plane reads it: the policy with the
//! overrides applied, and the route's rolling latency. Both live in the
//! snapshot, so a request that loaded the snapshot reads them without a lock.

use std::sync::Arc;

use fast_proxy_sampling::{LatencyWindow, SamplingPolicy};

/// How one route is sampled.
#[derive(Debug, Clone)]
pub struct CompiledSampling {
    pub policy: Arc<SamplingPolicy>,
    /// Shared with the next snapshot while the route exists.
    pub latency: Arc<LatencyWindow>,
}

impl CompiledSampling {
    /// Whether anything is done for this route at all. A request on a route
    /// where this is false touches no sampling state.
    pub fn is_on(&self) -> bool {
        self.policy.enabled
    }
}
