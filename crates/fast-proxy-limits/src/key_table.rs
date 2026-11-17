//! The per-policy table of keys and their GCRA times, bounded by a memory
//! budget every table shares (ADR 0030).
//!
//! The table is sharded so concurrent requests rarely wait on one lock, and a
//! lock is held for one lookup. When the budget is spent, the shard a new key
//! falls in drops the keys that are idle (their time has passed, so dropping
//! them changes no answer). If none is idle the request is **admitted** and
//! reported as [`Admission::TableFull`]: an attacker who fills the table must
//! not turn it into a denial of service for everyone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::{Gcra, Verdict};

const SHARDS: usize = 64;
/// A full shard looks for idle keys at most this often, so a flood of new keys
/// cannot make every request scan a shard.
const SWEEP_INTERVAL_NS: u64 = 100_000_000;

/// How many keys all tables together may hold; shared by every [`KeyTable`].
#[derive(Debug)]
pub struct KeyBudget {
    capacity: usize,
    used: AtomicUsize,
}

impl KeyBudget {
    pub fn new(capacity: usize) -> Arc<Self> {
        Arc::new(Self {
            capacity,
            used: AtomicUsize::new(0),
        })
    }

    /// Keys held by every table together.
    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    fn try_take(&self) -> bool {
        self.used
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
                (used < self.capacity).then_some(used + 1)
            })
            .is_ok()
    }

    fn release(&self, count: usize) {
        self.used.fetch_sub(count, Ordering::Relaxed);
    }
}

/// What the table decided for one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Allowed,
    Limited {
        retry_after: Duration,
    },
    /// Admitted without a check: the key is new and there was no room for it.
    TableFull,
}

#[derive(Default)]
struct Shard {
    times: HashMap<u64, u64>,
    last_sweep: u64,
}

/// Keys (already hashed, see [`crate::KeyHasher`]) and their GCRA times.
pub struct KeyTable {
    shards: Box<[Mutex<Shard>]>,
    budget: Arc<KeyBudget>,
}

impl std::fmt::Debug for KeyTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyTable")
            .field("keys", &self.len())
            .finish()
    }
}

impl KeyTable {
    pub fn new(budget: Arc<KeyBudget>) -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Mutex::default()).collect(),
            budget,
        }
    }

    /// Decides one request for `key` at `now` (nanoseconds, [`crate::monotonic_ns`]).
    pub fn check(&self, gcra: &Gcra, key: u64, now: u64) -> Admission {
        let mut shard = self.shard(key);
        if let Some(tat) = shard.times.get_mut(&key) {
            return match gcra.check(*tat, now) {
                Verdict::Allow { next_tat } => {
                    *tat = next_tat;
                    Admission::Allowed
                }
                Verdict::Deny { retry_after } => Admission::Limited { retry_after },
            };
        }
        // A key never seen is always allowed: its time is now.
        let Verdict::Allow { next_tat } = gcra.check(0, now) else {
            return Admission::Allowed;
        };
        if !self.budget.try_take() && !self.make_room(&mut shard, now) {
            return Admission::TableFull;
        }
        shard.times.insert(key, next_tat);
        Admission::Allowed
    }

    /// Keys this table holds.
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| lock(shard).times.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn shard(&self, key: u64) -> MutexGuard<'_, Shard> {
        lock(&self.shards[(key >> 58) as usize % SHARDS])
    }

    /// Drops this shard's idle keys and takes a place for a new one; `false`
    /// when there is still none (or the shard swept too recently to try).
    fn make_room(&self, shard: &mut Shard, now: u64) -> bool {
        if shard.last_sweep != 0 && now.saturating_sub(shard.last_sweep) < SWEEP_INTERVAL_NS {
            return false;
        }
        shard.last_sweep = now.max(1);
        let before = shard.times.len();
        shard.times.retain(|_, tat| !Gcra::is_idle(*tat, now));
        self.budget.release(before - shard.times.len());
        self.budget.try_take()
    }
}

impl Drop for KeyTable {
    fn drop(&mut self) {
        let held = self
            .shards
            .iter_mut()
            .map(|shard| {
                shard
                    .get_mut()
                    .unwrap_or_else(PoisonError::into_inner)
                    .times
                    .len()
            })
            .sum();
        self.budget.release(held);
    }
}

fn lock(shard: &Mutex<Shard>) -> MutexGuard<'_, Shard> {
    // A panic while holding the lock leaves plain numbers behind; keep using them.
    shard.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{RateLimitKey, RateLimitPolicy, DEFAULT_IPV6_PREFIX};

    use super::*;

    const SECOND: u64 = 1_000_000_000;

    fn one_per_second() -> Gcra {
        Gcra::new(&RateLimitPolicy {
            rate: 1,
            period: Duration::from_secs(1),
            burst: 1,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        })
    }

    /// Keys spread over every shard, as hashed keys are.
    fn key(n: u64) -> u64 {
        n.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }

    #[test]
    fn each_key_has_its_own_budget() {
        let table = KeyTable::new(KeyBudget::new(100));
        let gcra = one_per_second();
        assert_eq!(table.check(&gcra, key(1), SECOND), Admission::Allowed);
        assert_eq!(
            table.check(&gcra, key(1), SECOND),
            Admission::Limited {
                retry_after: Duration::from_secs(1)
            }
        );
        assert_eq!(table.check(&gcra, key(2), SECOND), Admission::Allowed);
    }

    #[test]
    fn the_budget_bounds_every_table_together() {
        let budget = KeyBudget::new(10);
        let a = KeyTable::new(budget.clone());
        let b = KeyTable::new(budget.clone());
        let gcra = one_per_second();
        for n in 0..6 {
            a.check(&gcra, key(n), SECOND);
            b.check(&gcra, key(100 + n), SECOND);
        }
        assert_eq!(budget.used(), 10);
        assert_eq!(a.len() + b.len(), 10);
    }

    #[test]
    fn a_full_table_admits_a_new_key_and_says_so() {
        let budget = KeyBudget::new(4);
        let table = KeyTable::new(budget.clone());
        let gcra = one_per_second();
        for n in 0..4 {
            assert_eq!(table.check(&gcra, key(n), SECOND), Admission::Allowed);
        }
        // Every held key is still busy, so nothing can be dropped.
        assert_eq!(table.check(&gcra, key(99), SECOND), Admission::TableFull);
        assert_eq!(table.len(), 4, "the table never grows past its budget");
        assert_eq!(
            table.check(&gcra, key(0), SECOND),
            Admission::Limited {
                retry_after: Duration::from_secs(1)
            },
            "keys it holds are still limited"
        );
    }

    #[test]
    fn idle_keys_make_room_for_new_ones() {
        let budget = KeyBudget::new(1);
        let table = KeyTable::new(budget.clone());
        let gcra = one_per_second();
        // Two keys in the same shard (the top bits choose it).
        let (first, second) = (1, 2);
        assert_eq!(table.check(&gcra, first, SECOND), Admission::Allowed);
        assert_eq!(table.check(&gcra, second, 3 * SECOND), Admission::Allowed);
        assert_eq!(table.len(), 1);
        assert_eq!(budget.used(), 1);
    }

    #[test]
    fn a_full_shard_sweeps_at_most_once_per_interval() {
        let table = KeyTable::new(KeyBudget::new(1));
        let gcra = one_per_second();
        assert_eq!(table.check(&gcra, 1, SECOND), Admission::Allowed);
        assert_eq!(
            table.check(&gcra, 2, SECOND),
            Admission::TableFull,
            "swept: nothing idle"
        );
        // Key 1 is idle now, but the shard swept a moment ago.
        let soon = 2 * SECOND;
        let mut shard = table.shard(1);
        shard.last_sweep = soon - 1;
        drop(shard);
        assert_eq!(table.check(&gcra, 3, soon), Admission::TableFull);
        assert_eq!(
            table.check(&gcra, 3, soon + SWEEP_INTERVAL_NS),
            Admission::Allowed
        );
    }

    #[test]
    fn a_dropped_table_returns_its_keys_to_the_budget() {
        let budget = KeyBudget::new(10);
        let table = KeyTable::new(budget.clone());
        for n in 0..3 {
            table.check(&one_per_second(), key(n), SECOND);
        }
        assert_eq!(budget.used(), 3);
        drop(table);
        assert_eq!(budget.used(), 0);
    }
}
