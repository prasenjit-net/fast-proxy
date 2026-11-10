use std::net::IpAddr;

use crate::FieldError;

/// An IP network such as `10.0.0.0/8` or `2001:db8::/32`; a bare address is a
/// single-host network. Host bits are masked off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    network: IpAddr,
    prefix_len: u8,
}

impl Cidr {
    /// Parses `address` or `address/prefix`; errors are reported against `field`.
    pub fn parse(field: &str, raw: &str) -> Result<Self, FieldError> {
        let invalid = |message: String| Err(FieldError::new(field, message));
        let (address, prefix) = match raw.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (raw, None),
        };
        let Ok(address) = address.parse::<IpAddr>() else {
            return invalid(format!("\"{raw}\" is not an IP address or network"));
        };
        let max = max_prefix_len(address);
        let prefix_len = match prefix {
            None => max,
            Some(text) => match text.parse::<u8>() {
                Ok(length) if length <= max => length,
                _ => return invalid(format!("\"{raw}\": the prefix must be 0 to {max}")),
            },
        };
        Ok(Self {
            network: mask(address, prefix_len),
            prefix_len,
        })
    }

    /// Whether `address` lies inside the network. IPv4-mapped IPv6 addresses
    /// count as their IPv4 form; the two families never match each other.
    pub fn contains(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        same_family(self.network, address) && mask(address, self.prefix_len) == self.network
    }

    /// The `address/prefix` spelling.
    pub fn to_cidr_string(&self) -> String {
        format!("{}/{}", self.network, self.prefix_len)
    }
}

fn max_prefix_len(address: IpAddr) -> u8 {
    match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    }
}

fn same_family(a: IpAddr, b: IpAddr) -> bool {
    a.is_ipv4() == b.is_ipv4()
}

fn mask(address: IpAddr, prefix_len: u8) -> IpAddr {
    match address {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let kept = u32::MAX
                .checked_shl(32 - u32::from(prefix_len))
                .unwrap_or(0);
            IpAddr::from((bits & kept).to_be_bytes())
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let kept = u128::MAX
                .checked_shl(128 - u32::from(prefix_len))
                .unwrap_or(0);
            IpAddr::from((bits & kept).to_be_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cidr(raw: &str) -> Cidr {
        Cidr::parse("bypass.cidrs", raw).unwrap()
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn a_network_contains_its_addresses_and_nothing_else() {
        let net = cidr("10.1.0.0/16");
        assert!(net.contains(ip("10.1.0.0")));
        assert!(net.contains(ip("10.1.255.255")));
        assert!(!net.contains(ip("10.2.0.0")));
        assert!(!net.contains(ip("9.255.255.255")));
    }

    #[test]
    fn a_bare_address_is_a_single_host() {
        let host = cidr("192.0.2.7");
        assert!(host.contains(ip("192.0.2.7")));
        assert!(!host.contains(ip("192.0.2.8")));
        assert_eq!(host.to_cidr_string(), "192.0.2.7/32");
    }

    #[test]
    fn host_bits_are_masked_off() {
        assert_eq!(cidr("10.1.2.3/8").to_cidr_string(), "10.0.0.0/8");
    }

    #[test]
    fn prefix_zero_matches_the_whole_family() {
        assert!(cidr("0.0.0.0/0").contains(ip("203.0.113.9")));
        assert!(!cidr("0.0.0.0/0").contains(ip("::1")));
        assert!(cidr("::/0").contains(ip("2001:db8::1")));
    }

    #[test]
    fn ipv6_networks_match_inside_and_outside() {
        let net = cidr("2001:db8::/32");
        assert!(net.contains(ip("2001:db8:ffff::1")));
        assert!(!net.contains(ip("2001:db9::1")));
        assert!(!net.contains(ip("192.0.2.1")));
    }

    #[test]
    fn an_ipv4_mapped_ipv6_client_matches_an_ipv4_network() {
        assert!(cidr("192.0.2.0/24").contains(ip("::ffff:192.0.2.9")));
    }

    #[test]
    fn rejects_garbage_and_out_of_range_prefixes() {
        for raw in [
            "",
            "nope",
            "10.0.0.0/33",
            "10.0.0.0/-1",
            "10.0.0.0/",
            "::/129",
            "1.2.3/8",
        ] {
            let err = Cidr::parse("bypass.cidrs", raw).unwrap_err();
            assert_eq!(err.field, "bypass.cidrs", "{raw:?}");
        }
    }
}
