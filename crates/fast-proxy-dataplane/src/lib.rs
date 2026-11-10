//! The hyper-based proxy data plane: listeners, the request pipeline, and
//! (in later tasks) the upstream client.
//!
//! The data plane only reads compiled snapshots from a
//! [`SnapshotHolder`](fast_proxy_routing::SnapshotHolder); it never touches
//! storage.

mod connections;
mod forward;
mod headers;
mod listener;
mod pending_request;
mod pipeline;
mod protocol_switch;
mod replay_body;
mod request_id;
mod responses;
mod retry_policy;
mod selection;
mod server;
mod tls_plane;
mod tls_resolver;
mod tracked_body;
mod trusted_proxies;
mod upstream_client;
mod upstream_errors;

#[cfg(test)]
mod accounting_tests;
#[cfg(test)]
mod balancing_tests;
#[cfg(test)]
mod drain_tests;
#[cfg(test)]
mod forwarding_tests;
#[cfg(test)]
mod observability_tests;
#[cfg(test)]
mod passive_health_tests;
#[cfg(test)]
mod retry_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_upstream;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod upgrade_tests;

pub use forward::{Forward, Forwarded, ResponseBody, Target};
pub use listener::{bind, BindError, BoundListener, ListenerSpec};
pub use pipeline::RequestPipeline;
pub use responses::FixedResponse;
pub use server::{DataPlane, DrainReport};
pub use tls_plane::{Http2Limits, TlsPlane, TlsPlaneError, TlsPlaneSettings};
pub use tracked_body::Observability;
pub use trusted_proxies::{TrustedProxies, TrustedProxyError};
pub use upstream_client::UpstreamClient;
