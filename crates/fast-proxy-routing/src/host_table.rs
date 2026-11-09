use std::collections::HashMap;

use fast_proxy_model::{HostPattern, SubsystemId};

/// Maps request `Host` values to subsystems: exact names first, then the
/// longest matching `*.suffix` wildcard.
#[derive(Debug, Clone, Default)]
pub struct HostTable {
    exact: HashMap<String, SubsystemId>,
    wildcard: HashMap<String, SubsystemId>,
}

impl HostTable {
    /// Builds the table. Patterns are assumed unique; a later duplicate
    /// replaces an earlier one (the compiler rejects duplicates up front).
    pub fn build<'a>(entries: impl IntoIterator<Item = (&'a HostPattern, SubsystemId)>) -> Self {
        let mut table = Self::default();
        for (pattern, id) in entries {
            match pattern {
                HostPattern::Exact(name) => table.exact.insert(name.clone(), id),
                HostPattern::Wildcard(suffix) => table.wildcard.insert(suffix.clone(), id),
            };
        }
        table
    }

    /// Finds the subsystem for a raw `Host` header value, or `None` if the
    /// header is missing, malformed, an IPv6 literal, or matches nothing.
    pub fn lookup(&self, host_header: &str) -> Option<SubsystemId> {
        let host = normalize_host(host_header)?;
        if let Some(id) = self.exact.get(&host) {
            return Some(*id);
        }
        let mut rest = host.as_str();
        while let Some((_, suffix)) = rest.split_once('.') {
            if let Some(id) = self.wildcard.get(suffix) {
                return Some(*id);
            }
            rest = suffix;
        }
        None
    }
}

/// Lowercases, strips a port and one trailing dot. Returns `None` for
/// values that can never match a DNS-name pattern.
fn normalize_host(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.starts_with('[') || raw.chars().any(char::is_whitespace) {
        return None;
    }
    let without_port = match raw.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return None,
        None => raw,
    };
    if without_port.contains(':') {
        return None;
    }
    let host = without_port.strip_suffix('.').unwrap_or(without_port);
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn pattern(raw: &str) -> HostPattern {
        HostPattern::parse(raw).unwrap()
    }

    fn table(entries: &[(&str, i64)]) -> HostTable {
        let parsed: Vec<(HostPattern, SubsystemId)> = entries
            .iter()
            .map(|(raw, id)| (pattern(raw), SubsystemId(*id)))
            .collect();
        HostTable::build(parsed.iter().map(|(p, id)| (p, *id)))
    }

    #[test]
    fn exact_names_match_case_insensitively_ignoring_port_and_trailing_dot() {
        let table = table(&[("shop.example.com", 1)]);
        for host in [
            "shop.example.com",
            "SHOP.Example.COM",
            "shop.example.com:8000",
            "shop.example.com.",
            "shop.example.com.:8000",
        ] {
            assert_eq!(table.lookup(host), Some(SubsystemId(1)), "{host}");
        }
    }

    #[test]
    fn wildcard_needs_at_least_one_extra_label() {
        let table = table(&[("*.example.com", 2)]);
        assert_eq!(table.lookup("a.example.com"), Some(SubsystemId(2)));
        assert_eq!(table.lookup("a.b.example.com"), Some(SubsystemId(2)));
        assert_eq!(table.lookup("example.com"), None);
        assert_eq!(table.lookup("badexample.com"), None);
    }

    #[test]
    fn exact_beats_a_wildcard_that_also_matches() {
        let table = table(&[("*.example.com", 2), ("www.example.com", 1)]);
        assert_eq!(table.lookup("www.example.com"), Some(SubsystemId(1)));
        assert_eq!(table.lookup("api.example.com"), Some(SubsystemId(2)));
    }

    #[test]
    fn the_longest_wildcard_suffix_wins() {
        let table = table(&[("*.example.com", 1), ("*.eu.example.com", 2)]);
        assert_eq!(table.lookup("a.eu.example.com"), Some(SubsystemId(2)));
        assert_eq!(table.lookup("a.us.example.com"), Some(SubsystemId(1)));
    }

    #[test]
    fn malformed_hosts_match_nothing_and_do_not_panic() {
        let table = table(&[("example.com", 1), ("*.example.com", 2)]);
        for host in [
            "",
            ":",
            ":80",
            ".",
            "[::1]",
            "[::1]:80",
            "::1",
            "a b",
            "example.com:",
            "a:b:c",
            "example.com:notaport",
        ] {
            assert_eq!(table.lookup(host), None, "{host:?}");
        }
    }

    #[test]
    fn an_empty_table_matches_nothing() {
        assert_eq!(HostTable::default().lookup("example.com"), None);
    }

    proptest! {
        #[test]
        fn an_exact_host_always_beats_a_matching_wildcard(
            labels in proptest::collection::vec("[a-z][a-z0-9]{0,5}", 2..5),
        ) {
            let host = labels.join(".");
            let suffix = labels[1..].join(".");
            let table = table(&[(&format!("*.{suffix}"), 1), (&host, 2)]);
            prop_assert_eq!(table.lookup(&host), Some(SubsystemId(2)));
        }

        #[test]
        fn the_longest_wildcard_wins_regardless_of_insertion_order(
            labels in proptest::collection::vec("[a-z][a-z0-9]{0,5}", 3..6),
            long_first in any::<bool>(),
        ) {
            let host = labels.join(".");
            let long = format!("*.{}", labels[1..].join("."));
            let short = format!("*.{}", labels[2..].join("."));
            let entries = if long_first {
                [(long.as_str(), 1), (short.as_str(), 2)]
            } else {
                [(short.as_str(), 2), (long.as_str(), 1)]
            };
            prop_assert_eq!(table(&entries).lookup(&host), Some(SubsystemId(1)));
        }

        #[test]
        fn matching_ignores_case(
            labels in proptest::collection::vec("[a-z][a-z0-9]{0,5}", 1..4),
        ) {
            let host = labels.join(".");
            let table = table(&[(&host, 1)]);
            prop_assert_eq!(table.lookup(&host.to_ascii_uppercase()), Some(SubsystemId(1)));
        }
    }
}
