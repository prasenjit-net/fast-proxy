use std::net::IpAddr;

use thiserror::Error;

/// A `trusted_proxies` entry could not be parsed.
#[derive(Debug, Error)]
#[error("invalid trusted proxy \"{entry}\": {reason}")]
pub struct TrustedProxyError {
    entry: String,
    reason: &'static str,
}

/// One network in CIDR notation, such as `10.0.0.0/8` or `::1/128`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cidr {
    network: IpAddr,
    prefix_len: u8,
}

impl Cidr {
    /// Parses `addr/prefix`; a bare address means a single host.
    fn parse(entry: &str) -> Result<Self, TrustedProxyError> {
        let fail = |reason| TrustedProxyError {
            entry: entry.to_string(),
            reason,
        };
        let (addr, prefix) = match entry.split_once('/') {
            Some((addr, prefix)) => (addr, Some(prefix)),
            None => (entry, None),
        };
        let network: IpAddr = addr.parse().map_err(|_| fail("not an IP address"))?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix_len = match prefix {
            None => max,
            Some(prefix) => prefix
                .parse::<u8>()
                .ok()
                .filter(|len| *len <= max)
                .ok_or_else(|| fail("prefix length out of range"))?,
        };
        Ok(Self {
            network,
            prefix_len,
        })
    }

    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => masked(
                u128::from(u32::from(network)),
                u128::from(u32::from(ip)),
                32,
                self.prefix_len,
            ),
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                masked(u128::from(network), u128::from(ip), 128, self.prefix_len)
            }
            _ => false,
        }
    }
}

/// True when the top `prefix_len` of `width` bits of both values agree.
fn masked(network: u128, ip: u128, width: u32, prefix_len: u8) -> bool {
    let shift = width - u32::from(prefix_len);
    shift == width || (network >> shift) == (ip >> shift)
}

/// The peers whose `X-Forwarded-*` headers are believed and appended to.
#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    networks: Vec<Cidr>,
}

impl TrustedProxies {
    /// Parses the config's CIDR list; fails on the first bad entry.
    pub fn parse(entries: &[String]) -> Result<Self, TrustedProxyError> {
        let networks = entries
            .iter()
            .map(|entry| Cidr::parse(entry))
            .collect::<Result<_, _>>()?;
        Ok(Self { networks })
    }

    /// The address to treat as the client: the peer itself, unless the peer is a
    /// trusted proxy, in which case the nearest `X-Forwarded-For` entry that is
    /// not itself a trusted proxy (read from the right, where each proxy
    /// appended what it saw, so a client cannot choose it by writing the
    /// header). Falls back to the peer when the header is absent or unusable.
    pub fn effective_client(&self, peer: IpAddr, headers: &http::HeaderMap) -> IpAddr {
        if !self.contains(peer) {
            return peer;
        }
        let entries = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .map(str::trim)
            .collect::<Vec<_>>();
        for entry in entries.into_iter().rev() {
            let Ok(ip) = entry.parse::<IpAddr>() else {
                return peer;
            };
            if !self.contains(ip) {
                return ip;
            }
        }
        peer
    }

    /// Whether `ip` (IPv4-mapped IPv6 addresses count as IPv4) is trusted.
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.networks.iter().any(|network| network.contains(ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(entries: &[&str]) -> TrustedProxies {
        let entries: Vec<String> = entries.iter().map(ToString::to_string).collect();
        TrustedProxies::parse(&entries).unwrap()
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn matches_ipv4_networks_on_the_prefix() {
        let proxies = trusted(&["10.0.0.0/8", "192.168.1.5"]);
        assert!(proxies.contains(ip("10.255.0.1")));
        assert!(proxies.contains(ip("192.168.1.5")));
        assert!(!proxies.contains(ip("11.0.0.1")));
        assert!(!proxies.contains(ip("192.168.1.6")));
    }

    #[test]
    fn matches_ipv6_networks_and_ipv4_mapped_peers() {
        let proxies = trusted(&["fd00::/8", "::1/128", "10.0.0.0/8"]);
        assert!(proxies.contains(ip("fd12::1")));
        assert!(proxies.contains(ip("::1")));
        assert!(!proxies.contains(ip("fe80::1")));
        assert!(proxies.contains(ip("::ffff:10.1.2.3")));
    }

    #[test]
    fn a_zero_prefix_trusts_every_address_of_that_family() {
        let proxies = trusted(&["0.0.0.0/0"]);
        assert!(proxies.contains(ip("203.0.113.9")));
        assert!(!proxies.contains(ip("2001:db8::1")));
    }

    #[test]
    fn an_empty_list_trusts_nobody() {
        assert!(!TrustedProxies::default().contains(ip("127.0.0.1")));
    }

    #[test]
    fn rejects_bad_entries_with_the_entry_in_the_message() {
        for entry in [
            "",
            "nope",
            "10.0.0.0/33",
            "10.0.0.0/x",
            "::1/129",
            "10.0.0.0/-1",
        ] {
            let err = TrustedProxies::parse(&[entry.to_string()]).unwrap_err();
            assert!(err.to_string().contains(entry), "{entry}: {err}");
        }
    }

    fn forwarded(values: &[&str]) -> http::HeaderMap {
        let mut headers = http::HeaderMap::new();
        for value in values {
            headers.append("x-forwarded-for", value.parse().unwrap());
        }
        headers
    }

    fn proxies() -> TrustedProxies {
        TrustedProxies::parse(&["10.0.0.0/8".to_string()]).unwrap()
    }

    #[test]
    fn an_untrusted_peer_is_always_itself() {
        let peer = ip("203.0.113.9");
        let client = proxies().effective_client(peer, &forwarded(&["1.2.3.4"]));
        assert_eq!(client, peer);
    }

    #[test]
    fn a_trusted_peer_stands_for_the_nearest_client_it_forwarded() {
        let client = proxies().effective_client(ip("10.0.0.1"), &forwarded(&["198.51.100.7"]));
        assert_eq!(client, ip("198.51.100.7"));
    }

    #[test]
    fn trusted_hops_are_skipped_from_the_right_and_left_entries_are_ignored() {
        let headers = forwarded(&["6.6.6.6, 198.51.100.7", "10.0.0.2"]);
        assert_eq!(
            proxies().effective_client(ip("10.0.0.1"), &headers),
            ip("198.51.100.7"),
            "what the client wrote on the left never wins"
        );
    }

    #[test]
    fn a_trusted_peer_without_a_usable_header_is_itself() {
        let peer = ip("10.0.0.1");
        for headers in [
            forwarded(&[]),
            forwarded(&["not-an-ip"]),
            forwarded(&["10.0.0.5, 10.0.0.6"]),
            forwarded(&["198.51.100.7, garbage"]),
        ] {
            assert_eq!(
                proxies().effective_client(peer, &headers),
                peer,
                "{headers:?}"
            );
        }
    }
}
