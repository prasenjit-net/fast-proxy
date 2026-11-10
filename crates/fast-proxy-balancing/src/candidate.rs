use std::sync::Arc;

use crate::EndpointRuntime;

/// One endpoint as the selector sees it: its live state, weight and the
/// operator's admin decision (`true` only for `enabled`).
#[derive(Debug, Clone)]
pub struct Candidate {
    pub runtime: Arc<EndpointRuntime>,
    pub weight: u32,
    pub admin_enabled: bool,
}

impl Candidate {
    /// Eligible for new requests: admin enabled, healthy and not ejected.
    pub fn is_eligible(&self, now_ms: u64) -> bool {
        self.admin_enabled && self.runtime.is_routable(now_ms)
    }
}

/// What selection needs to know about a request, extracted by the caller
/// (so no allocation happens here).
#[derive(Debug, Clone, Copy, Default)]
pub struct RequestContext {
    /// Hash of the consistent-hash key (see [`crate::hash_bytes`] and
    /// [`crate::hash_ip`]); `None` when the request has no key.
    pub key_hash: Option<u64>,
}

/// The chosen endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Index into the candidate slice.
    pub index: usize,
    /// No endpoint was eligible and the failure policy let the group serve
    /// anyway ("panic mode").
    pub panic: bool,
}
