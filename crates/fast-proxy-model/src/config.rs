use std::time::Duration;

use crate::{
    EndpointConfig, GroupPolicy, HostPattern, RetryPolicy, RoutePrefix, SubsystemName,
    UpstreamPrefix,
};

/// Database id of a subsystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SubsystemId(pub i64);

/// Database id of a host pattern row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostId(pub i64);

/// Database id of an upstream group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UpstreamGroupId(pub i64);

/// Database id of a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RouteId(pub i64);

/// The complete admin-managed configuration, as loaded from storage. Field
/// types are individually valid; relationships between them (duplicates,
/// references) are checked when the routing snapshot is compiled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub subsystems: Vec<SubsystemConfig>,
}

/// One subsystem: the hosts it answers to, its upstreams, and its routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubsystemConfig {
    pub id: SubsystemId,
    pub name: SubsystemName,
    pub enabled: bool,
    pub description: String,
    pub hosts: Vec<HostPattern>,
    pub upstream_groups: Vec<UpstreamGroupConfig>,
    pub routes: Vec<RouteConfig>,
}

/// A named set of endpoints with the policy that governs them: one to
/// [`MAX_ENDPOINTS_PER_GROUP`](crate::MAX_ENDPOINTS_PER_GROUP), checked when the
/// routing snapshot is compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamGroupConfig {
    pub id: UpstreamGroupId,
    pub name: String,
    pub policy: GroupPolicy,
    pub endpoints: Vec<EndpointConfig>,
}

/// A prefix route that forwards to an upstream group of the same subsystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteConfig {
    pub id: RouteId,
    pub prefix: RoutePrefix,
    pub upstream_group_id: UpstreamGroupId,
    pub strip_prefix: bool,
    pub upstream_prefix: UpstreamPrefix,
    pub preserve_host: bool,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub retry: RetryPolicy,
    pub enabled: bool,
}
