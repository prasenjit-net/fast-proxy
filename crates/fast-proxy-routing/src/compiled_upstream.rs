use std::sync::Arc;

use fast_proxy_balancing::{Balancer, Candidate, RetryBudget, RuntimeRegistry};
use fast_proxy_model::{
    parse_weight, EndpointAddr, EndpointId, EndpointState, GroupPolicy, UpstreamGroupConfig,
    UpstreamGroupId, MAX_ENDPOINTS_PER_GROUP,
};

/// One endpoint of a compiled group: its configuration (the live state is
/// in the index-aligned [`Candidate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledEndpoint {
    pub id: EndpointId,
    pub addr: EndpointAddr,
    pub weight: u32,
    pub state: EndpointState,
}

/// Why a group cannot be compiled, before the subsystem name is attached.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GroupFault {
    EndpointCount(usize),
    DuplicateEndpoint(String),
    Invalid(String),
}

/// An upstream group ready for request handling: its endpoints, their live
/// runtime (carried over from the previous snapshot), the policy, and the
/// balancer. Routes of the same group share one instance.
#[derive(Debug)]
pub struct CompiledUpstream {
    pub id: UpstreamGroupId,
    pub name: String,
    pub policy: GroupPolicy,
    /// Configuration view, in the same order as `candidates`.
    pub endpoints: Vec<CompiledEndpoint>,
    /// Live state per endpoint, in the same order as `endpoints`.
    pub candidates: Vec<Candidate>,
    pub balancer: Balancer,
    /// Caps this group's retries (see [`RetryBudget`]).
    pub retry_budget: RetryBudget,
    /// The TLS client configuration for the group's endpoints; `None` when the
    /// group is plain HTTP. Carried in the snapshot so a replaced CA bundle
    /// reaches new connections with the same atomic swap as everything else.
    pub client_tls: Option<Arc<rustls::ClientConfig>>,
}

impl CompiledUpstream {
    /// Compiles a group, taking each endpoint's runtime from `registry`.
    pub(crate) fn build(
        group: &UpstreamGroupConfig,
        registry: &RuntimeRegistry,
        client_tls: Option<Arc<rustls::ClientConfig>>,
    ) -> Result<Self, GroupFault> {
        let count = group.endpoints.len();
        if count == 0 || count > MAX_ENDPOINTS_PER_GROUP {
            return Err(GroupFault::EndpointCount(count));
        }
        group
            .policy
            .validate()
            .map_err(|err| GroupFault::Invalid(err.to_string()))?;
        let mut endpoints = Vec::with_capacity(count);
        let mut candidates = Vec::with_capacity(count);
        for endpoint in &group.endpoints {
            if endpoints
                .iter()
                .any(|seen: &CompiledEndpoint| seen.addr == endpoint.addr)
            {
                return Err(GroupFault::DuplicateEndpoint(endpoint.addr.to_string()));
            }
            parse_weight("weight", endpoint.weight)
                .map_err(|err| GroupFault::Invalid(err.to_string()))?;
            let runtime = registry
                .get(endpoint.id, &endpoint.addr)
                .ok_or_else(|| GroupFault::Invalid("endpoint has no runtime".to_string()))?;
            candidates.push(Candidate {
                runtime,
                weight: endpoint.weight,
                admin_enabled: endpoint.state == EndpointState::Enabled,
            });
            endpoints.push(CompiledEndpoint {
                id: endpoint.id,
                addr: endpoint.addr.clone(),
                weight: endpoint.weight,
                state: endpoint.state,
            });
        }
        let weights: Vec<_> = endpoints.iter().map(|e| (e.id, e.weight)).collect();
        Ok(Self {
            id: group.id,
            name: group.name.clone(),
            policy: group.policy.clone(),
            balancer: Balancer::new(&group.policy.lb, group.policy.failure, &weights),
            retry_budget: RetryBudget::new(group.policy.retry_budget),
            endpoints,
            candidates,
            client_tls,
        })
    }
}
