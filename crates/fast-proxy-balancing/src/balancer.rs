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
    /// The most requests one endpoint may have in flight (ADR 0031).
    max_requests: Option<u32>,
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
            max_requests: None,
        }
    }

    /// Caps each endpoint at `cap` requests in flight: a full endpoint is not
    /// chosen, not even in panic mode. `None` is unlimited.
    pub fn with_max_requests(mut self, cap: Option<u32>) -> Self {
        self.max_requests = cap;
        self
    }

    /// The per-endpoint cap, for the caller that counts the chosen request in
    /// flight ([`crate::EndpointRuntime::try_track`]).
    pub fn max_requests(&self) -> Option<u32> {
        self.max_requests
    }

    /// Whether a refusal is (at least partly) because endpoints are full: the
    /// group has a cap and an admin-enabled endpoint is at it. The data plane
    /// answers `503 endpoint_capacity` rather than "no healthy endpoint" then.
    pub fn is_at_capacity(&self, endpoints: &[Candidate]) -> bool {
        self.max_requests.is_some()
            && endpoints
                .iter()
                .any(|endpoint| endpoint.admin_enabled && !self.has_room(endpoint))
    }

    fn has_room(&self, endpoint: &Candidate) -> bool {
        self.max_requests
            .is_none_or(|cap| endpoint.runtime.in_flight() < cap)
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
    /// Draining and disabled endpoints are never chosen, and neither is an
    /// endpoint at the group's request cap. The chosen endpoint is marked
    /// selected (see [`crate::EndpointRuntime::mark_selected`]).
    pub fn select(
        &self,
        endpoints: &[Candidate],
        ctx: &RequestContext,
        exclude: Option<usize>,
        rng: &mut Rng,
        now_ms: u64,
    ) -> Option<Selection> {
        let allowed = |index: usize| Some(index) != exclude;
        let has_room = |index: usize| self.has_room(&endpoints[index]);
        let eligible = |index: usize| {
            allowed(index) && endpoints[index].is_eligible(now_ms) && has_room(index)
        };
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
        let admin_enabled =
            |index: usize| allowed(index) && endpoints[index].admin_enabled && has_room(index);
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
