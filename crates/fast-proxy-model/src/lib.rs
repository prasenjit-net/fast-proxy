//! Domain types and pure validation for fast-proxy configuration. No I/O lives here.
//!
//! Every type that wraps user input is constructed through a `parse` function
//! that validates and normalizes it, so a value of that type is always valid.

mod bounds;
mod certificate;
mod config;
mod endpoint;
mod endpoint_addr;
mod failure_policy;
mod field_error;
mod group_policy;
mod health_check;
mod host_pattern;
mod https_policy;
mod lb_policy;
mod passive_health;
mod path_prefix;
mod retry;
mod subsystem_name;
mod timeout;
mod upstream_tls;

pub use certificate::{
    covers, validate_san, CertificateId, CertificateMetadata, CertificateName, CertificateSource,
    MAX_CA_BUNDLES, MAX_CERTIFICATES, MAX_CHAIN_LENGTH, MAX_PEM_BYTES,
};
pub use config::{
    Config, HostId, RouteConfig, RouteId, SubsystemConfig, SubsystemId, UpstreamGroupConfig,
    UpstreamGroupId,
};
pub use endpoint::{
    parse_weight, EndpointConfig, EndpointId, EndpointState, MAX_ENDPOINTS_PER_GROUP,
};
pub use endpoint_addr::EndpointAddr;
pub use failure_policy::FailurePolicy;
pub use field_error::FieldError;
pub use group_policy::GroupPolicy;
pub use health_check::{HealthCheck, HealthKind};
pub use host_pattern::HostPattern;
pub use https_policy::{Hsts, HttpsPolicy, RedirectMode, MAX_HSTS_AGE_SECS};
pub use lb_policy::{HashKey, LbAlgorithm, LbPolicy};
pub use passive_health::PassiveHealth;
pub use path_prefix::{RoutePrefix, UpstreamPrefix};
pub use retry::{RetryBudget, RetryPolicy};
pub use subsystem_name::SubsystemName;
pub use timeout::parse_timeout_ms;
pub use upstream_tls::UpstreamTls;
