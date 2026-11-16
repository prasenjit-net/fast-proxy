//! Which hosts of a subsystem go into its ACME certificate (K5). One rule,
//! used by the runner, the API preview and the UI, so they cannot disagree.

use std::collections::HashSet;

use fast_proxy_model::{
    DomainAllowlist, HostPattern, SkipReason, SkippedName, MAX_NAMES_PER_CERTIFICATE,
};

/// The names an order covers, and every other host with the reason it is left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Eligibility {
    /// Exact names, in the order the hosts were given, at most 100.
    pub eligible: Vec<String>,
    pub skipped: Vec<SkippedName>,
}

/// Splits `hosts` into the names ACME can certify and the rest. A host is
/// eligible when it is an exact name with at least two labels, inside the
/// allowlist, and among the first 100 such names. Duplicates count once.
pub fn eligible_names(hosts: &[HostPattern], allowlist: &DomainAllowlist) -> Eligibility {
    let mut seen = HashSet::new();
    let mut result = Eligibility::default();
    for host in hosts {
        let written = host.to_pattern_string();
        if !seen.insert(written.clone()) {
            continue;
        }
        match reason_to_skip(host, allowlist, result.eligible.len()) {
            Some(reason) => result.skipped.push(SkippedName {
                name: written,
                reason,
            }),
            None => result.eligible.push(written),
        }
    }
    result
}

fn reason_to_skip(
    host: &HostPattern,
    allowlist: &DomainAllowlist,
    eligible_so_far: usize,
) -> Option<SkipReason> {
    let name = match host {
        HostPattern::Wildcard(_) => return Some(SkipReason::WildcardNeedsDns01),
        HostPattern::Exact(name) => name,
    };
    if !name.contains('.') {
        return Some(SkipReason::SingleLabel);
    }
    if !allowlist.allows(name) {
        return Some(SkipReason::NotAllowed);
    }
    if eligible_so_far >= MAX_NAMES_PER_CERTIFICATE {
        return Some(SkipReason::TooManyNames);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(list: &[&str]) -> Vec<HostPattern> {
        list.iter()
            .map(|h| HostPattern::parse(h).unwrap())
            .collect()
    }

    fn allow(list: &[&str]) -> DomainAllowlist {
        DomainAllowlist::parse(&list.iter().map(ToString::to_string).collect::<Vec<_>>()).unwrap()
    }

    fn skipped(name: &str, reason: SkipReason) -> SkippedName {
        SkippedName {
            name: name.into(),
            reason,
        }
    }

    #[test]
    fn each_kind_of_host_is_eligible_or_skipped_with_its_reason() {
        let result = eligible_names(
            &hosts(&[
                "shop.example.com",
                "*.shop.example.com",
                "localhost",
                "shop.other.net",
                "www.shop.example.com",
                "shop.example.com",
            ]),
            &allow(&["*.example.com"]),
        );
        assert_eq!(
            result.eligible,
            ["shop.example.com", "www.shop.example.com"]
        );
        assert_eq!(
            result.skipped,
            [
                skipped("*.shop.example.com", SkipReason::WildcardNeedsDns01),
                skipped("localhost", SkipReason::SingleLabel),
                skipped("shop.other.net", SkipReason::NotAllowed),
            ]
        );
    }

    #[test]
    fn names_past_the_hundredth_are_skipped_as_too_many() {
        let many: Vec<String> = (0..105).map(|i| format!("h{i}.example.com")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let result = eligible_names(&hosts(&refs), &allow(&["*.example.com"]));
        assert_eq!(result.eligible.len(), MAX_NAMES_PER_CERTIFICATE);
        assert_eq!(result.eligible.last().unwrap(), "h99.example.com");
        assert_eq!(result.skipped.len(), 5);
        assert!(result
            .skipped
            .iter()
            .all(|s| s.reason == SkipReason::TooManyNames));
    }

    #[test]
    fn no_hosts_means_nothing_to_order() {
        assert_eq!(
            eligible_names(&[], &allow(&["a.com"])),
            Eligibility::default()
        );
    }
}
