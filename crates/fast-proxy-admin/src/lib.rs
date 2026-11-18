//! The admin plane: REST API, WebSocket push, and the embedded single-page UI.

mod access_middleware;
mod acme_bridge;
mod api;
/// Entry points for the fuzz targets (ADR 0032); only with the `fuzzing` feature.
#[cfg(feature = "fuzzing")]
pub use api::fuzzing;
mod clock;
mod config_bridge;
mod endpoint_bridge;
mod error;
mod events;
mod guard;
mod host_allow_list;
mod host_check;
mod refusal_audit;
mod request_id;
mod route_table;
mod router;
mod runtime_info;
mod security;
mod security_headers;
mod state;
mod static_assets;
mod tls_server;
mod traffic;
mod ui_config;
mod ws;

#[cfg(test)]
mod acme_api_tests;
#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod audit_api_tests;
#[cfg(test)]
mod audit_trail_tests;
#[cfg(test)]
mod limits_api_tests;
#[cfg(test)]
mod maintenance_api_tests;
#[cfg(test)]
mod samples_api_tests;
#[cfg(test)]
mod samples_api_tests_support;
#[cfg(test)]
mod sampling_api_tests;
#[cfg(test)]
mod security_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tls_api_tests;
#[cfg(test)]
mod tls_server_tests;
#[cfg(test)]
mod ui_page_policy_tests;

pub use error::{AppError, AppResult};
pub use router::router;
pub use runtime_info::{LimitsInfo, ListenerInfo, RuntimeInfo, TlsInfo};
pub use security::{AdminSecurity, SecuritySettings};
pub use state::{AcmeAdmin, AdminState, SharedState};
pub use tls_server::{serve_tls, StoredCertificate};
pub use ui_config::UiConfig;

/// Starts the admin plane's background work: the live traffic sampler and
/// the bridges from applied config changes and endpoint state changes to the UI.
pub fn spawn_background_tasks(state: SharedState) {
    traffic::spawn_sampler(state.clone());
    config_bridge::spawn_config_bridge(state.clone());
    acme_bridge::spawn_acme_bridge(state.clone());
    endpoint_bridge::spawn_endpoint_bridge(state);
}
