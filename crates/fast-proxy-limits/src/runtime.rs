//! The rate-limit state that outlives a configuration swap: one key table per
//! policy, the key budget they share, and the process's key hasher. A recompile
//! keeps the table of every policy that still exists (so a reload never resets a
//! client's budget, and a changed rate applies from the next request) and drops
//! the tables of removed policies, returning their keys to the budget.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{KeyBudget, KeyHasher, KeyTable};

/// Which policy a table belongs to: a subsystem's (shared by its routes that
/// have no policy of their own) or one route's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PolicyIdentity {
    pub subsystem: String,
    /// The route prefix for a route's own policy; `None` for the subsystem's.
    pub route: Option<String>,
}

/// Key tables by policy, with what they share.
#[derive(Debug, Clone)]
pub struct RateLimitRuntime {
    budget: Arc<KeyBudget>,
    hasher: Arc<KeyHasher>,
    tables: Arc<HashMap<PolicyIdentity, Arc<KeyTable>>>,
}

/// How many keys all rate-limit tables hold together unless `[limits]
/// rate_limit_keys` says otherwise (ADR 0030): about 6 MiB of keys and times.
pub const DEFAULT_KEY_CAPACITY: usize = 100_000;

impl Default for RateLimitRuntime {
    fn default() -> Self {
        Self::new(DEFAULT_KEY_CAPACITY)
    }
}

impl RateLimitRuntime {
    /// No tables yet; all of them together may hold `capacity` keys.
    pub fn new(capacity: usize) -> Self {
        Self {
            budget: KeyBudget::new(capacity),
            hasher: Arc::new(KeyHasher::new()),
            tables: Arc::default(),
        }
    }

    /// The runtime for a configuration whose policies are `wanted`: existing
    /// tables are kept, new ones created, the rest dropped once nothing (the
    /// old snapshot) holds them. The old runtime is unchanged, so a compile that
    /// fails changes nothing.
    pub fn rebuilt_for(&self, wanted: impl IntoIterator<Item = PolicyIdentity>) -> Self {
        let tables = wanted
            .into_iter()
            .map(|identity| {
                let table = self
                    .tables
                    .get(&identity)
                    .cloned()
                    .unwrap_or_else(|| Arc::new(KeyTable::new(self.budget.clone())));
                (identity, table)
            })
            .collect();
        Self {
            budget: self.budget.clone(),
            hasher: self.hasher.clone(),
            tables: Arc::new(tables),
        }
    }

    /// The table of a policy this runtime was built for.
    pub fn table(&self, identity: &PolicyIdentity) -> Option<Arc<KeyTable>> {
        self.tables.get(identity).cloned()
    }

    pub fn hasher(&self) -> &Arc<KeyHasher> {
        &self.hasher
    }

    pub fn budget(&self) -> &Arc<KeyBudget> {
        &self.budget
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fast_proxy_model::{RateLimitKey, RateLimitPolicy, DEFAULT_IPV6_PREFIX};

    use super::*;
    use crate::{Admission, Gcra};

    fn identity(subsystem: &str, route: Option<&str>) -> PolicyIdentity {
        PolicyIdentity {
            subsystem: subsystem.into(),
            route: route.map(Into::into),
        }
    }

    fn gcra() -> Gcra {
        Gcra::new(&RateLimitPolicy {
            rate: 1,
            period: Duration::from_secs(60),
            burst: 1,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        })
    }

    #[test]
    fn a_rebuild_keeps_the_tables_of_policies_that_remain() {
        let first = RateLimitRuntime::new(100).rebuilt_for([identity("shop", None)]);
        let table = first.table(&identity("shop", None)).unwrap();
        assert_eq!(table.check(&gcra(), 7, 1), Admission::Allowed);

        let second = first.rebuilt_for([identity("shop", None), identity("shop", Some("/api"))]);
        let kept = second.table(&identity("shop", None)).unwrap();
        assert!(Arc::ptr_eq(&table, &kept), "the same table");
        assert!(
            matches!(kept.check(&gcra(), 7, 2), Admission::Limited { .. }),
            "the client's budget survived the swap"
        );
        assert!(second
            .table(&identity("shop", Some("/api")))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_removed_policy_returns_its_keys_once_the_old_snapshot_is_gone() {
        let old = RateLimitRuntime::new(100).rebuilt_for([identity("shop", None)]);
        old.table(&identity("shop", None))
            .unwrap()
            .check(&gcra(), 7, 1);
        let new = old.rebuilt_for([]);
        assert!(new.table(&identity("shop", None)).is_none());
        assert_eq!(new.budget().used(), 1, "the old snapshot still holds it");
        drop(old);
        assert_eq!(new.budget().used(), 0);
    }

    #[test]
    fn every_rebuild_shares_the_budget_and_the_hasher() {
        let first = RateLimitRuntime::new(5);
        let second = first.rebuilt_for([identity("a", None)]);
        assert!(Arc::ptr_eq(first.budget(), second.budget()));
        assert!(Arc::ptr_eq(first.hasher(), second.hasher()));
        assert_eq!(second.budget().capacity(), 5);
    }
}
