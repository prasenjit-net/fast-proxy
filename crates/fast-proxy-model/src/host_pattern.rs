use std::net::IpAddr;

use crate::FieldError;

const MAX_HOST_LEN: usize = 253;
const MAX_LABEL_LEN: usize = 63;

/// A host a subsystem answers to: an exact DNS name or `*.suffix`, which
/// matches any name with at least one extra label in front of the suffix.
/// Always stored lowercase.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HostPattern {
    Exact(String),
    /// The suffix without the leading `*.`.
    Wildcard(String),
}

impl HostPattern {
    /// Validates and lowercases `raw`. Rejects ports, trailing dots, IP
    /// literals, and wildcards anywhere but a single leading label.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let lowered = raw.to_ascii_lowercase();
        match lowered.strip_prefix("*.") {
            Some(suffix) => validate_dns_name(suffix).map(|()| Self::Wildcard(suffix.to_string())),
            None => validate_dns_name(&lowered).map(|()| Self::Exact(lowered)),
        }
    }

    /// The pattern as written in config: `name` or `*.suffix`.
    pub fn to_pattern_string(&self) -> String {
        match self {
            Self::Exact(name) => name.clone(),
            Self::Wildcard(suffix) => format!("*.{suffix}"),
        }
    }
}

/// Checks a DNS name per RFC 1123 labels; IP literals are rejected.
pub(crate) fn validate_dns_name(name: &str) -> Result<(), FieldError> {
    let invalid = |message: &str| Err(FieldError::new("host", message));
    if name.is_empty() {
        return invalid("must not be empty");
    }
    if name.len() > MAX_HOST_LEN {
        return invalid("must be at most 253 characters");
    }
    if name.contains(':') {
        return invalid("must not include a port");
    }
    if name.ends_with('.') {
        return invalid("must not end with a dot");
    }
    if name.parse::<IpAddr>().is_ok() {
        return invalid("IP addresses are not supported as host patterns");
    }
    for label in name.split('.') {
        if label.is_empty() || label.len() > MAX_LABEL_LEN {
            return invalid("each label must be 1 to 63 characters");
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return invalid(
                "may contain only letters, digits, '-' and '.', with '*' only as a leading '*.'",
            );
        }
        if label.starts_with('-') || label.ends_with('-') {
            return invalid("labels must not start or end with '-'");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_and_lowercases_exact_names() {
        for (raw, expected) in [
            ("shop.example.com", "shop.example.com"),
            ("Shop.Example.COM", "shop.example.com"),
            ("localhost", "localhost"),
            ("a-b.c1", "a-b.c1"),
        ] {
            assert_eq!(
                HostPattern::parse(raw).unwrap(),
                HostPattern::Exact(expected.into()),
                "{raw}"
            );
        }
    }

    #[test]
    fn accepts_a_leading_wildcard() {
        assert_eq!(
            HostPattern::parse("*.Example.com").unwrap(),
            HostPattern::Wildcard("example.com".into())
        );
    }

    #[test]
    fn rejects_invalid_patterns_with_a_host_field_error() {
        let long_label = format!("{}.com", "a".repeat(64));
        let long_name = format!("{}.com", ["a"; 130].join("."));
        for raw in [
            "",
            "*",
            "*.",
            "**.example.com",
            "*.*.example.com",
            "a.*.example.com",
            "shop*.example.com",
            "example.com:8080",
            "example.com.",
            "1.2.3.4",
            "::1",
            "[::1]",
            "-a.com",
            "a-.com",
            "a..com",
            ".a.com",
            "a_b.com",
            "a b.com",
            long_label.as_str(),
            long_name.as_str(),
        ] {
            let err = HostPattern::parse(raw).unwrap_err();
            assert_eq!(err.field, "host", "{raw:?}");
        }
    }

    #[test]
    fn round_trips_to_the_pattern_string() {
        for raw in ["shop.example.com", "*.example.com"] {
            assert_eq!(HostPattern::parse(raw).unwrap().to_pattern_string(), raw);
        }
    }
}
