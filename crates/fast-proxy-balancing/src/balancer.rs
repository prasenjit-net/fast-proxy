use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use fast_proxy_model::{EndpointId, FailurePolicy, HashKey, LbAlgorithm, LbPolicy};

use crate::algorithms::ring::Ring;
use crate::algorithms::{least_connections, p2c_ewma, round_robin, weighted_round_robin};
use crate::{Candidate, RequestContext, Rng, Selection};

enum Algorithm {
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    P2cEwma,
    ConsistentHash { ring: Ring, key: HashKey },
}

/// Chooses an endpoint of one group for each request. Built when the
/// configuration is compiled (so rings are never built per request) and
/// shared by every request through the snapshot.
pub struct Balancer {
    algorithm: Algorithm,
    failure_policy: FailurePolicy,
    counter: AtomicU64,
}

impl Balancer {
    /// Builds the balancer for a group whose endpoints are `(id, weight)` in
    /// the same order as the candidates later passed to [`Balancer::select`].
    pub fn new(
        lb: &LbPolicy,
        failure_policy: FailurePolicy,
        endpoints: &[(EndpointId, u32)],
    ) -> Self {
        let algorithm = match lb.algorithm {
            LbAlgorithm::RoundRobin => Algorithm::RoundRobin,
            LbAlgorithm::WeightedRoundRobin => Algorithm::WeightedRoundRobin,
            LbAlgorithm::LeastConnections => Algorithm::LeastConnections,
            LbAlgorithm::P2cEwma => Algorithm::P2cEwma,
            LbAlgorithm::ConsistentHash => Algorithm::ConsistentHash {
                ring: Ring::build(endpoints),
                key: lb.hash_key.clone(),
            },
        };
        Self {
            algorithm,
            failure_policy,
            counter: AtomicU64::new(0),
        }
    }

    /// What the caller must extract from the request and hash into
    /// [`RequestContext::key_hash`]; `None` unless the group hashes.
    pub fn hash_key(&self) -> Option<&HashKey> {
        match &self.algorithm {
            Algorithm::ConsistentHash { key, .. } => Some(key),
            _ => None,
        }
    }

    /// Picks an endpoint, never choosing `exclude` (the one that just failed).
    /// Only admin-enabled, healthy, non-ejected endpoints are eligible. With
    /// none eligible, a `fail_open` group picks among admin-enabled endpoints
    /// ignoring health (`panic` is set); `fail_closed` returns `None`.
    /// Draining and disabled endpoints are never chosen. The chosen endpoint is
    /// marked selected (see [`crate::EndpointRuntime::mark_selected`]).
    pub fn select(
        &self,
        endpoints: &[Candidate],
        ctx: &RequestContext,
        exclude: Option<usize>,
        rng: &mut Rng,
        now_ms: u64,
    ) -> Option<Selection> {
        let allowed = |index: usize| Some(index) != exclude;
        let eligible = |index: usize| allowed(index) && endpoints[index].is_eligible(now_ms);
        if let Some(index) = self.pick(endpoints, ctx, rng, &eligible) {
            endpoints[index].runtime.mark_selected(now_ms);
            return Some(Selection {
                index,
                panic: false,
            });
        }
        if self.failure_policy == FailurePolicy::FailClosed {
            return None;
        }
        let admin_enabled = |index: usize| allowed(index) && endpoints[index].admin_enabled;
        let index = self.pick(endpoints, ctx, rng, &admin_enabled)?;
        Some(Selection { index, panic: true })
    }

    fn pick(
        &self,
        endpoints: &[Candidate],
        ctx: &RequestContext,
        rng: &mut Rng,
        usable: &impl Fn(usize) -> bool,
    ) -> Option<usize> {
        match &self.algorithm {
            Algorithm::RoundRobin => round_robin::pick(&self.counter, endpoints.len(), usable),
            Algorithm::WeightedRoundRobin => {
                weighted_round_robin::pick(&self.counter, endpoints, usable)
            }
            Algorithm::LeastConnections => least_connections::pick(endpoints, rng, usable),
            Algorithm::P2cEwma => p2c_ewma::pick(endpoints, rng, usable),
            Algorithm::ConsistentHash { ring, .. } => match ctx.key_hash {
                Some(key) => ring.lookup(key, usable),
                None => round_robin::pick(&self.counter, endpoints.len(), usable),
            },
        }
    }
}

impl std::fmt::Debug for Balancer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Balancer")
            .field("failure_policy", &self.failure_policy)
            .field("counter", &self.counter.load(Relaxed))
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
