use std::collections::HashMap;
use std::time::Duration;

use fast_proxy_model::{EndpointAddr, RouteId, SubsystemId, SubsystemName, UpstreamPrefix};

use crate::{HostTable, RouteTable};

/// A route with its upstream resolved to a concrete endpoint address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledRoute {
    pub id: RouteId,
    pub strip_prefix: bool,
    pub upstream_prefix: UpstreamPrefix,
    pub preserve_host: bool,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub upstream_name: String,
    pub endpoint: EndpointAddr,
}

/// One subsystem ready for request handling. Disabled routes are not
/// present; a disabled subsystem is kept so its hosts still resolve (to a 503).
#[derive(Debug, Clone)]
pub struct CompiledSubsystem {
    pub name: SubsystemName,
    pub enabled: bool,
    pub routes: RouteTable<CompiledRoute>,
}

/// The immutable, ready-to-serve view of the whole configuration at one
/// version. The data plane reads it lock-free and never mutates it.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub version: u64,
    pub hosts: HostTable,
    pub subsystems: HashMap<SubsystemId, CompiledSubsystem>,
}

impl Snapshot {
    /// The snapshot before any configuration exists: version 0, no hosts.
    pub fn empty() -> Self {
        Self::default()
    }
}
