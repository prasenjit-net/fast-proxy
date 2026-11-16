//! A route's rate limit, compiled (ADR 0030): the algorithm, what it keys on, and
//! the key table it shares with every route under the same policy.

use std::net::IpAddr;
use std::sync::Arc;

use fast_proxy_limits::{Admission, Gcra, KeyHasher, KeyTable, PolicyIdentity};
use fast_proxy_model::{RateLimitKey, RateLimitPolicy};

/// The rate limit that applies to a route: its own policy, or its subsystem's.
#[derive(Debug, Clone)]
pub struct CompiledRateLimit {
    /// Which policy this is; routes under the subsystem's policy share it.
    pub identity: PolicyIdentity,
    gcra: Gcra,
    key: RateLimitKey,
    ipv6_prefix: u8,
    table: Arc<KeyTable>,
    hasher: Arc<KeyHasher>,
}

impl CompiledRateLimit {
    pub(crate) fn new(
        identity: PolicyIdentity,
        policy: &RateLimitPolicy,
        table: Arc<KeyTable>,
        hasher: Arc<KeyHasher>,
    ) -> Self {
        Self {
            identity,
            gcra: Gcra::new(policy),
            key: policy.key.clone(),
            ipv6_prefix: policy.ipv6_prefix,
            table,
            hasher,
        }
    }

    /// The header whose value is the key, for a policy keyed on one; the caller
    /// passes its value to [`CompiledRateLimit::check`].
    pub fn key_header(&self) -> Option<&str> {
        match &self.key {
            RateLimitKey::Header(name) => Some(name.as_str()),
            RateLimitKey::Client => None,
        }
    }

    /// Decides one request from the effective client address and, for a header
    /// policy, the header's value (the client address when it is missing), at
    /// `now` from [`fast_proxy_limits::monotonic_ns`].
    pub fn check(&self, client: IpAddr, header_value: Option<&[u8]>, now: u64) -> Admission {
        let key = match (&self.key, header_value) {
            (RateLimitKey::Header(_), Some(value)) => self.hasher.header(value),
            _ => self.hasher.client(client, self.ipv6_prefix),
        };
        self.table.check(&self.gcra, key, now)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use fast_proxy_limits::{KeyBudget, PolicyIdentity};
    use fast_proxy_model::{RateLimitHeader, DEFAULT_IPV6_PREFIX};

    use super::*;

    fn limit(key: RateLimitKey) -> CompiledRateLimit {
        let policy = RateLimitPolicy {
            rate: 1,
            period: Duration::from_secs(60),
            burst: 1,
            key,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        };
        CompiledRateLimit::new(
            PolicyIdentity {
                subsystem: "shop".into(),
                route: None,
            },
            &policy,
            Arc::new(KeyTable::new(KeyBudget::new(100))),
            Arc::new(KeyHasher::new()),
        )
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_client_policy_limits_each_address() {
        let limit = limit(RateLimitKey::Client);
        assert_eq!(limit.key_header(), None);
        assert_eq!(limit.check(ip("192.0.2.1"), None, 1), Admission::Allowed);
        assert!(matches!(
            limit.check(ip("192.0.2.1"), Some(b"ignored"), 2),
            Admission::Limited { .. }
        ));
        assert_eq!(limit.check(ip("192.0.2.2"), None, 2), Admission::Allowed);
    }

    #[test]
    fn a_header_policy_limits_each_value_and_falls_back_to_the_address() {
        let header = RateLimitHeader::parse("h", "x-api-key").unwrap();
        let limit = limit(RateLimitKey::Header(header));
        assert_eq!(limit.key_header(), Some("x-api-key"));
        let client = ip("192.0.2.1");
        assert_eq!(limit.check(client, Some(b"a"), 1), Admission::Allowed);
        assert_eq!(
            limit.check(client, Some(b"b"), 1),
            Admission::Allowed,
            "another key"
        );
        assert!(matches!(
            limit.check(client, Some(b"a"), 2),
            Admission::Limited { .. }
        ));
        assert_eq!(
            limit.check(client, None, 2),
            Admission::Allowed,
            "the address"
        );
        assert!(matches!(
            limit.check(client, None, 3),
            Admission::Limited { .. }
        ));
    }
}
