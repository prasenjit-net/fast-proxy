use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    RetryPolicy, RouteId, SubsystemId, SubsystemName, UpstreamGroupId, UpstreamPrefix,
};

use crate::{CompiledHttps, CompiledMaintenance, CompiledUpstream, HostTable, RouteTable};

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
}

impl Snapshot {
    /// The snapshot before any configuration exists: version 0, no hosts.
    pub fn empty() -> Self {
        Self::default()
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
