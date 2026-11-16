use serde::Deserialize;

/// The `[limits]` section: process limits (ADR 0029), changed by a restart.
/// Rate-limit *policies* are configuration in the database, per subsystem and
/// route; this section only bounds what they may cost the process.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// How many client keys every rate-limit table together may hold. When it is
    /// reached, idle keys make room; if none is idle a new client is let through
    /// and counted (fail open), so the table cannot be turned against everyone.
    pub rate_limit_keys: usize,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            rate_limit_keys: fast_proxy_limits::DEFAULT_KEY_CAPACITY,
        }
    }
}
