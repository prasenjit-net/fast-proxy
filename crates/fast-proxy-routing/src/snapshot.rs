use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_limits::RateLimitRuntime;
use fast_proxy_model::{
    RetryPolicy, RouteId, SamplingSettings, SubsystemId, SubsystemName, UpstreamGroupId,
    UpstreamPrefix,
};
use fast_proxy_sampling::SamplingPolicy;

use crate::{
    AcmeChallenges, CompiledHttps, CompiledMaintenance, CompiledRateLimit, CompiledSampling,
    CompiledUpstream, HostTable, RouteTable, SamplingRuntime,
};

/// A route with its upstream group resolved. The endpoint is chosen per
/// request from [`CompiledRoute::upstream`].
#[derive(Debug, Clone)]
pub struct CompiledRoute {
    pub id: RouteId,
    /// The path prefix this route matches; used for metric labels and logs.
    pub prefix: String,
    pub strip_prefix: bool,
    pub upstream_prefix: UpstreamPrefix,
    pub preserve_host: bool,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub retry: RetryPolicy,
    /// Shared by every route of the same group.
    pub upstream: Arc<CompiledUpstream>,
    /// Maintenance set on this route alone, while active.
    pub maintenance: Option<Arc<CompiledMaintenance>>,
    /// How requests on this route are sampled.
    pub sampling: CompiledSampling,
    /// The rate limit that applies (the route's own policy, or its subsystem's).
    pub rate_limit: Option<CompiledRateLimit>,
    /// The largest request body; `None` uses the process default.
    pub max_body_bytes: Option<u64>,
}

/// One subsystem ready for request handling. Disabled routes are not
/// present; a disabled subsystem is kept so its hosts still resolve (to a 503).
#[derive(Debug, Clone)]
pub struct CompiledSubsystem {
    pub name: SubsystemName,
    pub enabled: bool,
    pub https: CompiledHttps,
    pub routes: RouteTable<CompiledRoute>,
    /// Every upstream group of the subsystem, including ones no enabled
    /// route uses (their runtime state must survive too).
    pub upstreams: HashMap<UpstreamGroupId, Arc<CompiledUpstream>>,
    /// Maintenance set on the whole subsystem, while active.
    pub maintenance: Option<Arc<CompiledMaintenance>>,
    /// How requests are sampled when no route of this subsystem took them.
    pub sampling: Arc<SamplingPolicy>,
}

/// The immutable, ready-to-serve view of the whole configuration at one
/// version. The data plane reads it lock-free and never mutates it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub version: u64,
    /// Whether TLS is on for the process; subsystem HTTPS settings are inert
    /// while it is off.
    pub tls_enabled: bool,
    pub hosts: HostTable,
    pub subsystems: HashMap<SubsystemId, CompiledSubsystem>,
    /// Maintenance set on the whole proxy, while active.
    pub maintenance: Option<Arc<CompiledMaintenance>>,
    /// The global sampling settings this snapshot was compiled from (the
    /// redactor's additions come from here).
    pub sampling_settings: Arc<SamplingSettings>,
    /// How requests no subsystem took are sampled.
    pub unmatched_sampling: Arc<SamplingPolicy>,
    /// The process-wide sampling state and every route's rolling latency.
    pub sampling: SamplingRuntime,
    /// The pending ACME HTTP-01 challenges, answered ahead of routing.
    pub acme_challenges: Arc<AcmeChallenges>,
    /// The rate-limit key tables of every policy, carried to the next compile.
    pub rate_limits: RateLimitRuntime,
}

impl Snapshot {
    /// The snapshot before any configuration exists: version 0, no hosts.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The sampling state to hand to the next compile, so a configuration
    /// change resets neither the guards nor a route's rolling latency.
    pub fn sampling_runtime(&self) -> SamplingRuntime {
        self.sampling.clone()
    }

    /// The rate-limit state to pass to the next compile, so tables of policies
    /// that remain keep their keys.
    pub fn rate_limit_runtime(&self) -> RateLimitRuntime {
        self.rate_limits.clone()
    }

    /// The runtime state of every endpoint in this snapshot; the next
    /// compile carries it over (see [`crate::compile`]).
    pub fn runtime_registry(&self) -> RuntimeRegistry {
        RuntimeRegistry::from_runtimes(
            self.subsystems
                .values()
                .flat_map(|subsystem| subsystem.upstreams.values())
                .flat_map(|upstream| {
                    upstream.endpoints.iter().zip(&upstream.candidates).map(
                        |(endpoint, candidate)| {
                            (
                                endpoint.id,
                                endpoint.addr.clone(),
                                candidate.runtime.clone(),
                            )
                        },
                    )
                }),
        )
    }
}
