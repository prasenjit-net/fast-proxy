//! Builders shared by the crate's unit tests.

use std::sync::Arc;

use crate::{Candidate, EndpointRuntime};

/// Enabled, healthy candidates with the given weights.
pub fn candidates(weights: &[u32]) -> Vec<Candidate> {
    weights
        .iter()
        .map(|weight| Candidate {
            runtime: Arc::new(EndpointRuntime::new()),
            weight: *weight,
            admin_enabled: true,
        })
        .collect()
}
