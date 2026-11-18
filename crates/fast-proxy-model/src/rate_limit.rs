//! A rate-limit policy on a subsystem or a route (ADR 0030): how many requests
//! one key may make in a period, with a burst allowance, and what the key is.

use std::time::Duration;

use crate::bounds::check_range;
use crate::FieldError;

/// The most requests a policy can allow per period.
pub const MAX_RATE: u32 = 1_000_000;
/// The longest period, in seconds (one hour).
pub const MAX_PERIOD_SECS: u32 = 3_600;
/// The largest burst a policy can allow.
pub const MAX_BURST: u32 = 10_000;
/// The longest header name a header key may use.
const MAX_HEADER_NAME_LEN: usize = 64;
/// IPv6 clients are keyed by this many leading bits unless a policy says otherwise.
pub const DEFAULT_IPV6_PREFIX: u8 = 64;

/// Headers that describe the connection or the framing, never a caller: keying
/// on one would limit by something the proxy itself rewrites.
const RESERVED_HEADERS: &[&str] = &[
    "connection",
    "content-length",
    "host",
    "keep-alive",
    "proxy-authenticate",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// What a policy counts requests by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RateLimitKey {
    /// The effective client address (`X-Forwarded-For` only from a trusted proxy).
    Client,
    /// The value of one request header (for example an API key), falling back to
    /// the client address when the request does not carry it.
    Header(RateLimitHeader),
}

/// A header name a policy may key on: a lower-case HTTP token that is not a
/// connection or framing header.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RateLimitHeader(String);

impl RateLimitHeader {
    /// Validates `name`, reporting errors against `field`.
    pub fn parse(field: &str, name: &str) -> Result<Self, FieldError> {
        let lower = name.trim().to_ascii_lowercase();
        if lower.is_empty() || lower.len() > MAX_HEADER_NAME_LEN {
            return Err(FieldError::new(
                field,
                format!("must be a header name of 1 to {MAX_HEADER_NAME_LEN} characters"),
            ));
        }
        if !lower.bytes().all(is_token_byte) {
            return Err(FieldError::new(field, "must be a valid header name"));
        }
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err(FieldError::new(
                field,
                format!("cannot be `{lower}`: it describes the connection, not the caller"),
            ));
        }
        Ok(Self(lower))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `tchar` of RFC 9110: what a header name may contain.
fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

/// At most `rate` requests per `period` for one key, with up to `burst` of them
/// at once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitPolicy {
    pub rate: u32,
    pub period: Duration,
    pub burst: u32,
    pub key: RateLimitKey,
    /// How many leading bits of an IPv6 client address form its key.
    pub ipv6_prefix: u8,
}

impl RateLimitPolicy {
    /// Checks every bound; errors name the field in the API's spelling.
    pub fn validate(&self) -> Result<(), FieldError> {
        check_range("rateLimit.rate", self.rate, 1, MAX_RATE)?;
        let period = u32::try_from(self.period.as_secs()).unwrap_or(u32::MAX);
        if self.period.subsec_nanos() != 0 {
            return Err(FieldError::new(
                "rateLimit.periodSecs",
                "must be whole seconds",
            ));
        }
        check_range("rateLimit.periodSecs", period, 1, MAX_PERIOD_SECS)?;
        check_range("rateLimit.burst", self.burst, 1, MAX_BURST)?;
        check_range("rateLimit.ipv6Prefix", u32::from(self.ipv6_prefix), 48, 128)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> RateLimitPolicy {
        RateLimitPolicy {
            rate: 100,
            period: Duration::from_secs(1),
            burst: 20,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        }
    }

    #[test]
    fn a_sensible_policy_is_valid() {
        assert!(policy().validate().is_ok());
    }

    /// Breaks one bound of a valid policy.
    type Breaker = fn(&mut RateLimitPolicy);

    #[test]
    fn each_bound_is_reported_with_its_field() {
        let cases: [(Breaker, &str); 7] = [
            (|p| p.rate = 0, "rateLimit.rate"),
            (|p| p.rate = MAX_RATE + 1, "rateLimit.rate"),
            (|p| p.period = Duration::ZERO, "rateLimit.periodSecs"),
            (
                |p| p.period = Duration::from_millis(1500),
                "rateLimit.periodSecs",
            ),
            (
                |p| p.period = Duration::from_secs(3601),
                "rateLimit.periodSecs",
            ),
            (|p| p.burst = 0, "rateLimit.burst"),
            (|p| p.ipv6_prefix = 32, "rateLimit.ipv6Prefix"),
        ];
        for (break_it, field) in cases {
            let mut policy = policy();
            break_it(&mut policy);
            assert_eq!(policy.validate().unwrap_err().field, field);
        }
    }

    #[test]
    fn a_header_name_is_lower_cased_and_must_be_a_token() {
        let header = RateLimitHeader::parse("rateLimit.header", " X-Api-Key ").unwrap();
        assert_eq!(header.as_str(), "x-api-key");
        for bad in ["", "x api", "x:y", "é", &"a".repeat(65)] {
            assert!(RateLimitHeader::parse("f", bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn connection_and_framing_headers_cannot_be_keys() {
        for reserved in ["Host", "Connection", "transfer-encoding", "Upgrade"] {
            let err = RateLimitHeader::parse("rateLimit.header", reserved).unwrap_err();
            assert_eq!(err.field, "rateLimit.header");
        }
    }
}
