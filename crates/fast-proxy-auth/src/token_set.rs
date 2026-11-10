use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwap;
use fast_proxy_model::{Role, SecretHash, TokenId, TokenName};
use thiserror::Error;

use crate::principal::Principal;
use crate::secret::{hash_secret, secret_matches};
use crate::token::ParsedToken;

/// What the set needs to know about one stored token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecord {
    pub id: TokenId,
    pub name: TokenName,
    pub role: Role,
    pub secret_hash: SecretHash,
    pub expires_at_ms: Option<i64>,
    pub revoked: bool,
}

/// Why authentication failed. Callers answer all of them with the same 401;
/// the distinction is for the audit log only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AuthError {
    #[error("malformed token")]
    Malformed,
    #[error("unknown token")]
    Unknown,
    #[error("wrong secret")]
    BadSecret,
    #[error("token revoked")]
    Revoked,
    #[error("token expired")]
    Expired,
}

/// An immutable set of tokens, built from stored rows.
#[derive(Debug, Default)]
pub struct TokenSet {
    records: HashMap<TokenId, Arc<TokenRecord>>,
}

impl TokenSet {
    /// Builds a set from `records`, sharing the entries of `previous` that did
    /// not change so a rebuild after one new token copies almost nothing.
    pub fn build(records: Vec<TokenRecord>, previous: Option<&TokenSet>) -> Self {
        let records = records
            .into_iter()
            .map(|record| {
                let shared = previous
                    .and_then(|set| set.records.get(&record.id))
                    .filter(|old| ***old == record)
                    .cloned();
                (record.id, shared.unwrap_or_else(|| Arc::new(record)))
            })
            .collect();
        Self { records }
    }

    /// Checks a presented `fp_<id>.<secret>` at `now_ms`.
    ///
    /// The secret is verified before revocation and expiry are looked at, and
    /// an unknown id costs the same hash and comparison, so a caller without
    /// the secret learns nothing about which tokens exist or what state they
    /// are in.
    pub fn authenticate(&self, raw: &str, now_ms: i64) -> Result<Principal, AuthError> {
        let presented = ParsedToken::parse(raw).map_err(|_| AuthError::Malformed)?;
        let Some(record) = self.records.get(&presented.id) else {
            // Do the same work as for a known token.
            let _ = secret_matches(presented.secret, &hash_secret(""));
            return Err(AuthError::Unknown);
        };
        if !secret_matches(presented.secret, &record.secret_hash) {
            return Err(AuthError::BadSecret);
        }
        if record.revoked {
            return Err(AuthError::Revoked);
        }
        if record
            .expires_at_ms
            .is_some_and(|expires| expires <= now_ms)
        {
            return Err(AuthError::Expired);
        }
        Ok(Principal {
            token_id: record.id,
            name: record.name.as_str().to_string(),
            role: record.role,
            expires_at_ms: record.expires_at_ms,
        })
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// The live token set, swapped atomically when tokens change so requests never
/// wait on a rebuild.
#[derive(Debug, Default)]
pub struct TokenCache {
    current: ArcSwap<TokenSet>,
}

impl TokenCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the set with one built from `records`, reusing unchanged entries.
    pub fn replace(&self, records: Vec<TokenRecord>) {
        let next = TokenSet::build(records, Some(&self.current.load()));
        self.current.store(Arc::new(next));
    }

    /// See [`TokenSet::authenticate`].
    pub fn authenticate(&self, raw: &str, now_ms: i64) -> Result<Principal, AuthError> {
        self.current.load().authenticate(raw, now_ms)
    }

    pub fn len(&self) -> usize {
        self.current.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.current.load().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GeneratedSecret, PlaintextToken};

    struct Issued {
        record: TokenRecord,
        token: String,
    }

    fn issue(id: i64, role: Role) -> Issued {
        let secret = GeneratedSecret::generate().unwrap();
        let token = PlaintextToken::new(TokenId(id), &secret)
            .reveal()
            .to_string();
        Issued {
            record: TokenRecord {
                id: TokenId(id),
                name: TokenName::parse(&format!("token-{id}")).unwrap(),
                role,
                secret_hash: secret.hash().clone(),
                expires_at_ms: None,
                revoked: false,
            },
            token,
        }
    }

    fn set_of(issued: &[&Issued]) -> TokenSet {
        TokenSet::build(issued.iter().map(|i| i.record.clone()).collect(), None)
    }

    #[test]
    fn a_valid_token_yields_its_principal() {
        let admin = issue(1, Role::Admin);
        let principal = set_of(&[&admin]).authenticate(&admin.token, 0).unwrap();
        assert_eq!(principal.token_id, TokenId(1));
        assert_eq!(principal.name, "token-1");
        assert_eq!(principal.role, Role::Admin);
        assert_eq!(principal.expires_at_ms, None);
    }

    #[test]
    fn every_failure_has_its_own_reason() {
        let good = issue(1, Role::Viewer);
        let other = issue(2, Role::Viewer);
        let set = set_of(&[&good]);
        let wrong_secret = format!("fp_1.{}", &other.token[5..]);
        let truncated = &good.token[..good.token.len() - 1];
        assert_eq!(set.authenticate("", 0), Err(AuthError::Malformed));
        assert_eq!(set.authenticate("fp_x.y", 0), Err(AuthError::Malformed));
        assert_eq!(set.authenticate(&other.token, 0), Err(AuthError::Unknown));
        assert_eq!(
            set.authenticate(&wrong_secret, 0),
            Err(AuthError::BadSecret)
        );
        assert_eq!(set.authenticate(truncated, 0), Err(AuthError::BadSecret));
    }

    #[test]
    fn a_revoked_token_is_refused() {
        let mut revoked = issue(1, Role::Admin);
        revoked.record.revoked = true;
        let set = set_of(&[&revoked]);
        assert_eq!(set.authenticate(&revoked.token, 0), Err(AuthError::Revoked));
    }

    #[test]
    fn expiry_applies_at_the_exact_millisecond_without_any_write() {
        let mut expiring = issue(1, Role::Admin);
        expiring.record.expires_at_ms = Some(1_000);
        let set = set_of(&[&expiring]);
        assert!(set.authenticate(&expiring.token, 999).is_ok());
        assert_eq!(
            set.authenticate(&expiring.token, 1_000),
            Err(AuthError::Expired)
        );
        assert_eq!(
            set.authenticate(&expiring.token, 5_000),
            Err(AuthError::Expired)
        );
    }

    #[test]
    fn a_wrong_secret_does_not_reveal_that_the_token_is_revoked_or_expired() {
        let mut revoked = issue(1, Role::Admin);
        revoked.record.revoked = true;
        let set = set_of(&[&revoked]);
        assert_eq!(
            set.authenticate("fp_1.not-the-secret", 0),
            Err(AuthError::BadSecret)
        );
    }

    #[test]
    fn a_rebuild_reuses_unchanged_entries_and_replaces_changed_ones() {
        let first = issue(1, Role::Admin);
        let second = issue(2, Role::Viewer);
        let before = set_of(&[&first, &second]);

        let mut revoked_second = second.record.clone();
        revoked_second.revoked = true;
        let third = issue(3, Role::Viewer);
        let after = TokenSet::build(
            vec![first.record.clone(), revoked_second, third.record.clone()],
            Some(&before),
        );

        assert!(Arc::ptr_eq(
            &before.records[&TokenId(1)],
            &after.records[&TokenId(1)]
        ));
        assert!(!Arc::ptr_eq(
            &before.records[&TokenId(2)],
            &after.records[&TokenId(2)]
        ));
        assert_eq!(
            after.authenticate(&second.token, 0),
            Err(AuthError::Revoked)
        );
        assert!(after.authenticate(&third.token, 0).is_ok());
    }

    #[test]
    fn the_cache_follows_create_and_revoke_without_being_rebuilt_by_callers() {
        let cache = TokenCache::new();
        assert!(cache.is_empty());
        let admin = issue(1, Role::Admin);
        let viewer = issue(2, Role::Viewer);
        assert_eq!(cache.authenticate(&admin.token, 0), Err(AuthError::Unknown));

        cache.replace(vec![admin.record.clone(), viewer.record.clone()]);
        assert_eq!(cache.len(), 2);
        assert!(cache.authenticate(&viewer.token, 0).is_ok());

        let mut revoked = viewer.record.clone();
        revoked.revoked = true;
        cache.replace(vec![admin.record.clone(), revoked]);
        assert_eq!(
            cache.authenticate(&viewer.token, 0),
            Err(AuthError::Revoked)
        );
        assert!(cache.authenticate(&admin.token, 0).is_ok());
    }

    #[test]
    fn a_thousand_tokens_verify_quickly() {
        let all: Vec<Issued> = (1..=1000).map(|id| issue(id, Role::Viewer)).collect();
        let set = TokenSet::build(all.iter().map(|i| i.record.clone()).collect(), None);
        let started = std::time::Instant::now();
        for issued in &all {
            assert!(set.authenticate(&issued.token, 0).is_ok());
        }
        // A generous bound (a debug build on a loaded CI machine): the point is
        // a hash lookup per token, not a scan.
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
