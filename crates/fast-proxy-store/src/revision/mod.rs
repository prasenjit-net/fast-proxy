//! Revision history: every committed change stores the full resulting
//! configuration, and any revision can be restored.

mod config_json;
mod history;
mod restore;

pub use history::{append, current_version, get, list, Revision, RevisionSummary};
pub use restore::restore;

#[cfg(test)]
mod tests;
