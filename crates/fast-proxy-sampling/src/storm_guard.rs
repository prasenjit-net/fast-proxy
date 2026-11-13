//! The storm guard (decision J19). A connectivity failure can fire for every
//! request at once, so before the global rate limit each candidate passes a
//! per-key limiter: a burst of a few samples, then one per interval. A key is
//! the route, the trigger and, for connectivity triggers, the endpoint and the
//! failure kind, so one dead endpoint is one key. What was suppressed is
//! counted and handed to the next kept sample of that key, so the table still
//! says how large the outage was.
//!
//! The table is fixed size and nothing is allocated per call. Two keys can
//! share a slot (a collision only merges their budgets).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use fast_proxy_model::Trigger;

use crate::bucket::{take, UNSET};
use crate::facts::FailureKind;

const SLOTS: usize = 1024;

/// Identifies the thing a storm is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StormKey(u64);

impl StormKey {
    /// Builds the key for a candidate sample. `endpoint` names the endpoint
    /// (its position in its group) and the failure kind for the connectivity
    /// triggers, and is `None` for the others.
    pub fn new(
        subsystem: &str,
        route: &str,
        trigger: Trigger,
        endpoint: Option<(u16, FailureKind)>,
    ) -> Self {
        let mut hash = Fnv::new();
        hash.write(subsystem.as_bytes());
        hash.write(&[0]);
        hash.write(route.as_bytes());
        hash.write(&[0]);
        hash.write(trigger.as_str().as_bytes());
        if let Some((index, kind)) = endpoint {
            hash.write(&index.to_le_bytes());
            hash.write(kind.as_str().as_bytes());
        }
        Self(hash.finish())
    }
}

/// FNV-1a: a fixed hash, so slots are the same on every run.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// What the guard says about one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// Keep it. `suppressed` candidates of this key were dropped since the last
    /// kept one, the first `since_ms` milliseconds ago.
    Keep { suppressed: u32, since_ms: u32 },
    /// Drop it; it has been counted.
    Suppress,
}

/// The per-key limiter.
#[derive(Debug)]
pub struct StormGuard {
    credit: Box<[AtomicU64]>,
    suppressed: Box<[AtomicU32]>,
    first_suppressed_ms: Box<[AtomicU32]>,
}

impl Default for StormGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl StormGuard {
    pub fn new() -> Self {
        Self {
            credit: (0..SLOTS).map(|_| AtomicU64::new(UNSET)).collect(),
            suppressed: (0..SLOTS).map(|_| AtomicU32::new(0)).collect(),
            first_suppressed_ms: (0..SLOTS).map(|_| AtomicU32::new(0)).collect(),
        }
    }

    /// Decides one candidate: the first `burst` of a key are kept, then one per
    /// `interval_secs`. A key that has been quiet for `burst` intervals has its
    /// whole burst back.
    pub fn admit(&self, key: StormKey, burst: u32, interval_secs: u32, now_ms: u64) -> Admission {
        let slot = (key.0 % SLOTS as u64) as usize;
        let interval_ms = interval_secs.saturating_mul(1000).max(1);
        let cap = burst.saturating_mul(interval_ms);
        let kept = take(
            &self.credit[slot],
            now_ms,
            (cap, cap),
            |elapsed_ms| elapsed_ms,
            interval_ms,
        );
        if kept {
            let suppressed = self.suppressed[slot].swap(0, Ordering::Relaxed);
            let since_ms = (now_ms as u32)
                .wrapping_sub(self.first_suppressed_ms[slot].load(Ordering::Relaxed));
            return Admission::Keep {
                suppressed,
                since_ms: if suppressed == 0 { 0 } else { since_ms },
            };
        }
        if self.suppressed[slot].fetch_add(1, Ordering::Relaxed) == 0 {
            self.first_suppressed_ms[slot].store(now_ms as u32, Ordering::Relaxed);
        }
        Admission::Suppress
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn key(route: &str) -> StormKey {
        StormKey::new(
            "shop",
            route,
            Trigger::UpstreamFailure,
            Some((0, FailureKind::ConnectRefused)),
        )
    }

    fn kept(admission: Admission) -> bool {
        matches!(admission, Admission::Keep { .. })
    }

    #[test]
    fn a_burst_is_kept_then_one_per_interval() {
        let guard = StormGuard::new();
        let k = key("/api");
        let first: Vec<bool> = (0..6).map(|_| kept(guard.admit(k, 3, 30, 0))).collect();
        assert_eq!(first, [true, true, true, false, false, false]);
        assert!(!kept(guard.admit(k, 3, 30, 29_999)));
        assert!(
            kept(guard.admit(k, 3, 30, 30_000)),
            "one interval buys one sample"
        );
        assert!(!kept(guard.admit(k, 3, 30, 30_001)));
    }

    #[test]
    fn one_key_does_not_spend_another_keys_budget() {
        let guard = StormGuard::new();
        for _ in 0..10 {
            guard.admit(key("/api"), 3, 30, 0);
        }
        assert!(kept(guard.admit(key("/other"), 3, 30, 0)));
        let other_kind = StormKey::new(
            "shop",
            "/api",
            Trigger::UpstreamFailure,
            Some((0, FailureKind::ResponseTimeout)),
        );
        assert!(
            kept(guard.admit(other_kind, 3, 30, 0)),
            "a different failure kind is a different key"
        );
        let other_endpoint = StormKey::new(
            "shop",
            "/api",
            Trigger::UpstreamFailure,
            Some((1, FailureKind::ConnectRefused)),
        );
        assert!(
            kept(guard.admit(other_endpoint, 3, 30, 0)),
            "a different endpoint is a different key"
        );
        let other_trigger = StormKey::new("shop", "/api", Trigger::Slow, None);
        assert!(kept(guard.admit(other_trigger, 3, 30, 0)));
    }

    #[test]
    fn the_suppressed_count_arrives_on_the_next_kept_sample_exactly_once() {
        let guard = StormGuard::new();
        let k = key("/api");
        for _ in 0..3 {
            assert_eq!(
                guard.admit(k, 3, 30, 1_000),
                Admission::Keep {
                    suppressed: 0,
                    since_ms: 0
                }
            );
        }
        for _ in 0..40 {
            assert_eq!(guard.admit(k, 3, 30, 5_000), Admission::Suppress);
        }
        assert_eq!(
            guard.admit(k, 3, 30, 31_000),
            Admission::Keep {
                suppressed: 40,
                since_ms: 26_000
            }
        );
        assert_eq!(
            guard.admit(k, 3, 30, 61_000),
            Admission::Keep {
                suppressed: 0,
                since_ms: 0
            }
        );
    }

    #[test]
    fn a_quiet_period_restores_the_burst() {
        let guard = StormGuard::new();
        let k = key("/api");
        for _ in 0..10 {
            guard.admit(k, 3, 30, 0);
        }
        let again = (0..6)
            .filter(|_| kept(guard.admit(k, 3, 30, 3_600_000)))
            .count();
        assert_eq!(again, 3);
    }

    #[test]
    fn the_burst_and_interval_are_read_on_every_call() {
        let guard = StormGuard::new();
        let k = key("/api");
        assert!(kept(guard.admit(k, 1, 10, 0)));
        assert!(!kept(guard.admit(k, 1, 10, 0)));
        assert!(
            kept(guard.admit(k, 1, 1, 1_000)),
            "a shorter interval takes effect at once"
        );
    }

    #[test]
    fn colliding_keys_share_a_budget_without_panicking() {
        let guard = StormGuard::new();
        let a = StormKey(7);
        let b = StormKey(7 + SLOTS as u64);
        for _ in 0..3 {
            assert!(kept(guard.admit(a, 3, 30, 0)));
        }
        assert!(
            !kept(guard.admit(b, 3, 30, 0)),
            "the second key finds the slot spent"
        );
    }

    #[test]
    fn concurrent_callers_never_exceed_the_burst() {
        let guard = Arc::new(StormGuard::new());
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let guard = guard.clone();
                std::thread::spawn(move || {
                    (0..500)
                        .filter(|_| kept(guard.admit(key("/api"), 5, 30, 0)))
                        .count()
                })
            })
            .collect();
        let total: usize = threads.into_iter().map(|t| t.join().unwrap()).sum();
        assert_eq!(total, 5);
    }

    #[test]
    fn the_key_ignores_nothing_it_should_notice() {
        let a = StormKey::new("a", "/x", Trigger::Slow, None);
        assert_eq!(a, StormKey::new("a", "/x", Trigger::Slow, None));
        assert_ne!(a, StormKey::new("b", "/x", Trigger::Slow, None));
        assert_ne!(a, StormKey::new("a", "/y", Trigger::Slow, None));
        assert_ne!(
            StormKey::new("ab", "c", Trigger::Slow, None),
            StormKey::new("a", "bc", Trigger::Slow, None),
            "the separator keeps field boundaries"
        );
    }
}
