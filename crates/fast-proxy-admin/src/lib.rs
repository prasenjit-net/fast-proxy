//! The admin plane: REST API, WebSocket push, and the embedded single-page UI.

mod access_middleware;
mod api;
mod error;
mod events;
mod live_metrics;
mod router;
mod runtime_info;
mod state;
mod static_assets;
mod ui_config;
mod ws;

#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use error::{AppError, AppResult};
pub use live_metrics::spawn_sampler;
pub use router::router;
pub use runtime_info::{ListenerInfo, RuntimeInfo};
pub use state::{AdminState, SharedState};
pub use ui_config::UiConfig;
