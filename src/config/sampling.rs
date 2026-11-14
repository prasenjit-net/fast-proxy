use std::time::Duration;

use serde::Deserialize;

use super::duration::deserialize_duration;

/// The `[sampling]` section: process settings of request sampling. What is
/// sampled, the rates and the retention are shared by every instance and live in
/// the database, changed through the admin plane (ADR 0023).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SamplingConfig {
    /// The master switch; off removes every cost of sampling. `--sampling` and
    /// `--no-sampling` override it.
    pub enabled: bool,
    /// How many kept samples may wait to be written.
    pub queue_depth: usize,
    /// Samples written to the database in one transaction, at most.
    pub batch_size: usize,
    /// The longest a kept sample waits for its batch.
    #[serde(deserialize_with = "deserialize_duration")]
    pub batch_interval: Duration,
    /// How often this instance prunes the shared sample table (retention).
    #[serde(deserialize_with = "deserialize_duration")]
    pub prune_interval: Duration,
    /// The most memory, in bytes, that captured bodies may hold at once.
    pub body_budget_bytes: u64,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            queue_depth: 1024,
            batch_size: 50,
            batch_interval: Duration::from_millis(250),
            prune_interval: Duration::from_secs(30),
            body_budget_bytes: 16 * 1024 * 1024,
        }
    }
}
