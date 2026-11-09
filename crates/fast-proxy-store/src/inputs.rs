use std::time::Duration;

use fast_proxy_model::{EndpointAddr, RoutePrefix, SubsystemName, UpstreamGroupId, UpstreamPrefix};

/// The editable fields of a subsystem.
#[derive(Debug, Clone)]
pub struct SubsystemFields {
    pub name: SubsystemName,
    pub enabled: bool,
    pub description: String,
}

/// The editable fields of an upstream group and its single endpoint (M1).
#[derive(Debug, Clone)]
pub struct UpstreamFields {
    pub name: String,
    pub addr: EndpointAddr,
    pub weight: u32,
}

/// The editable fields of a route.
#[derive(Debug, Clone)]
pub struct RouteFields {
    pub prefix: RoutePrefix,
    pub upstream_group_id: UpstreamGroupId,
    pub strip_prefix: bool,
    pub upstream_prefix: UpstreamPrefix,
    pub preserve_host: bool,
    pub connect_timeout: Duration,
    pub response_timeout: Duration,
    pub enabled: bool,
}
