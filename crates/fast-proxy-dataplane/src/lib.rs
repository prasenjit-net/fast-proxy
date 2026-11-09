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
mod pipeline;
mod request_id;
mod responses;
mod server;
mod trusted_proxies;
mod upstream_client;
mod upstream_uri;

#[cfg(test)]
mod forwarding_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod test_upstream;
#[cfg(test)]
mod tests;

pub use forward::{Forward, ResponseBody, Target};
pub use listener::{bind, BindError, BoundListener, ListenerSpec};
pub use pipeline::RequestPipeline;
pub use responses::FixedResponse;
pub use server::DataPlane;
pub use trusted_proxies::{TrustedProxies, TrustedProxyError};
pub use upstream_client::UpstreamClient;
