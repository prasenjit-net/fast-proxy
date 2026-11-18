//! Turns what a policy keys on into the 64-bit key a [`crate::KeyTable`] holds.
//! The hash is seeded randomly per process, so nobody outside can aim keys at
//! one shard, and the table never holds an address or a header value itself.

use std::hash::{BuildHasher, Hasher};
use std::net::IpAddr;

/// Distinguishes the kinds of key, so a header value can never collide with an
/// address by construction.
const CLIENT_V4: u8 = 4;
const CLIENT_V6: u8 = 6;
const HEADER: u8 = b'h';

/// A seeded hash for rate-limit keys, one per process.
#[derive(Debug, Clone, Default)]
pub struct KeyHasher {
    seed: std::collections::hash_map::RandomState,
}

impl KeyHasher {
    pub fn new() -> Self {
        Self::default()
    }

    /// The key of a client address. An IPv6 address is cut to its first
    /// `ipv6_prefix` bits, so one host cannot rotate through its own prefix.
    pub fn client(&self, address: IpAddr, ipv6_prefix: u8) -> u64 {
        let mut hasher = self.seed.build_hasher();
        match address {
            IpAddr::V4(v4) => {
                hasher.write_u8(CLIENT_V4);
                hasher.write(&v4.octets());
            }
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => {
                    hasher.write_u8(CLIENT_V4);
                    hasher.write(&v4.octets());
                }
                None => {
                    let bits = u128::from(v6);
                    let prefix = u32::from(ipv6_prefix.min(128));
                    let masked = bits & u128::MAX.checked_shl(128 - prefix).unwrap_or(0);
                    hasher.write_u8(CLIENT_V6);
                    hasher.write(&masked.to_be_bytes());
                }
            },
        }
        hasher.finish()
    }

    /// The key of a header value (for example an API key).
    pub fn header(&self, value: &[u8]) -> u64 {
        let mut hasher = self.seed.build_hasher();
        hasher.write_u8(HEADER);
        hasher.write(value);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn ipv6_clients_in_one_prefix_share_a_key() {
        let hasher = KeyHasher::new();
        let a = hasher.client(ip("2001:db8:1:2::1"), 64);
        let b = hasher.client(ip("2001:db8:1:2:ffff::9"), 64);
        let other = hasher.client(ip("2001:db8:1:3::1"), 64);
        assert_eq!(a, b);
        assert_ne!(a, other);
        assert_ne!(
            hasher.client(ip("2001:db8:1:2::1"), 128),
            hasher.client(ip("2001:db8:1:2::2"), 128),
            "a /128 policy keys each address"
        );
    }

    #[test]
    fn an_ipv4_mapped_address_is_the_ipv4_client() {
        let hasher = KeyHasher::new();
        assert_eq!(
            hasher.client(ip("::ffff:192.0.2.7"), 64),
            hasher.client(ip("192.0.2.7"), 64)
        );
        assert_ne!(
            hasher.client(ip("192.0.2.7"), 64),
            hasher.client(ip("192.0.2.8"), 64)
        );
    }

    #[test]
    fn a_header_value_never_shares_a_key_with_an_address() {
        let hasher = KeyHasher::new();
        let address = ip("192.0.2.7");
        let IpAddr::V4(v4) = address else {
            unreachable!()
        };
        assert_ne!(hasher.header(&v4.octets()), hasher.client(address, 64));
        assert_eq!(hasher.header(b"key-1"), hasher.header(b"key-1"));
        assert_ne!(hasher.header(b"key-1"), hasher.header(b"key-2"));
    }

    #[test]
    fn two_processes_hash_differently() {
        let (a, b) = (KeyHasher::new(), KeyHasher::new());
        assert_ne!(
            a.header(b"same"),
            b.header(b"same"),
            "each has its own seed"
        );
    }
}
