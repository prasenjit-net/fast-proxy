use std::time::Duration;

use fast_proxy_model::{
    EndpointAddr, EndpointState, GroupPolicy, HttpsPolicy, RetryPolicy, RoutePrefix, SubsystemName,
    UpstreamGroupId, UpstreamPrefix,
};

/// The editable fields of a subsystem.
#[derive(Debug, Clone)]
pub struct SubsystemFields {
    pub name: SubsystemName,
    pub enabled: bool,
    pub description: String,
    pub https: HttpsPolicy,
}

/// The editable settings of an upstream group (its endpoints are separate).
#[derive(Debug, Clone)]
pub struct UpstreamFields {
    pub name: String,
    pub policy: GroupPolicy,
}

/// The editable fields of one endpoint.
#[derive(Debug, Clone)]
pub struct EndpointFields {
    pub addr: EndpointAddr,
    pub weight: u32,
    pub state: EndpointState,
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
    pub retry: RetryPolicy,
    pub enabled: bool,
}
