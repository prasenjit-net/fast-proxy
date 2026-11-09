//! The config apply pipeline. Every admin change goes through
//! [`ConfigService::apply`], which validates it against the compiled routing
//! rules, persists it with a revision, and swaps the live snapshot, in that
//! order and under one writer lock. The data plane only ever sees the holder.

mod error;
mod service;

pub use error::StartError;
pub use service::{Applied, ConfigChanged, ConfigService};

#[cfg(test)]
mod tests;
