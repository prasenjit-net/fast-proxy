//! A short id per admin request, for the audit log and `X-Request-Id`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// The id of the current request, stored in the request's extensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestId(pub String);

static COUNTER: AtomicU64 = AtomicU64::new(1);
static PROCESS_PREFIX: OnceLock<String> = OnceLock::new();

impl RequestId {
    /// A new id, unique within this process: its start time and a counter.
    pub fn next() -> Self {
        let prefix = PROCESS_PREFIX
            .get_or_init(|| format!("{:x}", chrono::Utc::now().timestamp_millis().unsigned_abs()));
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(format!("{prefix}-{n:x}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_share_a_process_prefix() {
        let a = RequestId::next();
        let b = RequestId::next();
        assert_ne!(a, b);
        assert_eq!(a.0.split('-').next(), b.0.split('-').next());
    }
}
