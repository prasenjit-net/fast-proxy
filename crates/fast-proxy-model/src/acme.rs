//! The ACME vocabulary (M4): the domain allowlist, the certificate key type,
//! challenge tokens, and the per-subsystem status. No protocol and no I/O live
//! here; the ACME crate and the runner build on these types.

use std::fmt;

use crate::{CertificateId, FieldError, HostPattern, SubsystemId};

/// Most characters of an ACME challenge token (RFC 8555 asks for at least 128
/// bits of entropy; CAs send 43 characters).
pub const MAX_TOKEN_LEN: usize = 128;
/// Most characters of a key authorization: the token, a dot and a SHA-256
/// thumbprint in base64url (43 characters).
pub const MAX_KEY_AUTHORIZATION_LEN: usize = MAX_TOKEN_LEN + 1 + 64;
/// Most names in one ACME certificate (the Let's Encrypt limit).
pub const MAX_NAMES_PER_CERTIFICATE: usize = 100;
/// Most patterns in the domain allowlist.
pub const MAX_ALLOWED_DOMAINS: usize = 256;
/// Most characters of a stored error detail.
pub const MAX_ERROR_DETAIL_CHARS: usize = 300;

/// The names ACME may issue for, from `[data_plane.acme] allowed_domains`.
/// `example.com` allows exactly that name; `*.example.com` allows any name
/// below it, like a host pattern (it does not allow `example.com` itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainAllowlist(Vec<HostPattern>);

impl DomainAllowlist {
    /// Parses every entry with the host-pattern rules; empty or too long lists
    /// are refused so a typo cannot open issuance to every name.
    pub fn parse(entries: &[String]) -> Result<Self, FieldError> {
        const FIELD: &str = "allowed_domains";
        if entries.is_empty() {
            return Err(FieldError::new(FIELD, "must list at least one domain"));
        }
        if entries.len() > MAX_ALLOWED_DOMAINS {
            return Err(FieldError::new(FIELD, "must list at most 256 domains"));
        }
        entries
            .iter()
            .map(|entry| {
                HostPattern::parse(entry)
                    .map_err(|err| FieldError::new(FIELD, format!("{entry:?}: {}", err.message)))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Self)
    }

    /// Whether `name` (a lowercase DNS name) may be certified.
    pub fn allows(&self, name: &str) -> bool {
        self.0.iter().any(|pattern| match pattern {
            HostPattern::Exact(exact) => exact == name,
            HostPattern::Wildcard(suffix) => name
                .strip_suffix(suffix.as_str())
                .and_then(|front| front.strip_suffix('.'))
                .is_some_and(|front| !front.is_empty()),
        })
    }

    /// The entries as written, for status and logs.
    pub fn patterns(&self) -> Vec<String> {
        self.0.iter().map(HostPattern::to_pattern_string).collect()
    }
}

/// The key algorithm of issued certificates. `ring` cannot generate RSA keys,
/// so only ECDSA is offered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AcmeKeyType {
    #[default]
    EcdsaP256,
    EcdsaP384,
}

impl AcmeKeyType {
    /// The config spelling: `ecdsa-p256` or `ecdsa-p384`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EcdsaP256 => "ecdsa-p256",
            Self::EcdsaP384 => "ecdsa-p384",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "ecdsa-p256" => Ok(Self::EcdsaP256),
            "ecdsa-p384" => Ok(Self::EcdsaP384),
            _ => Err(FieldError::new(
                "key_type",
                "must be ecdsa-p256 or ecdsa-p384",
            )),
        }
    }
}

/// Why a host of an ACME subsystem is not put in its certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SkipReason {
    /// A wildcard host needs DNS-01, which is not supported.
    WildcardNeedsDns01,
    /// A name with one label (`localhost`) cannot be publicly certified.
    SingleLabel,
    /// The name is outside `allowed_domains`.
    NotAllowed,
    /// The certificate already holds the most names a CA accepts.
    TooManyNames,
}

impl SkipReason {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WildcardNeedsDns01 => "wildcard_needs_dns01",
            Self::SingleLabel => "single_label",
            Self::NotAllowed => "not_allowed",
            Self::TooManyNames => "too_many_names",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "wildcard_needs_dns01" => Ok(Self::WildcardNeedsDns01),
            "single_label" => Ok(Self::SingleLabel),
            "not_allowed" => Ok(Self::NotAllowed),
            "too_many_names" => Ok(Self::TooManyNames),
            _ => Err(FieldError::new("reason", "unknown skip reason")),
        }
    }
}

/// A host left out of a certificate, with the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedName {
    /// The host pattern as written (`*.example.com` for a wildcard).
    pub name: String,
    pub reason: SkipReason,
}

/// Where a subsystem's ACME certificate stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AcmeState {
    /// Nothing to do: no eligible name.
    #[default]
    Idle,
    /// The first certificate is being ordered (the development one serves).
    Pending,
    /// A current ACME certificate is attached and not due.
    Valid,
    /// A current certificate serves while its replacement is ordered.
    Renewing,
    /// The last order failed; the next attempt is scheduled.
    Failed,
    /// The CA asked us to wait; the next attempt is at the time it named.
    RateLimited,
    /// ACME is on for the subsystem but off for the process.
    Inactive,
}

impl AcmeState {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Pending => "pending",
            Self::Valid => "valid",
            Self::Renewing => "renewing",
            Self::Failed => "failed",
            Self::RateLimited => "rate_limited",
            Self::Inactive => "inactive",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "idle" => Ok(Self::Idle),
            "pending" => Ok(Self::Pending),
            "valid" => Ok(Self::Valid),
            "renewing" => Ok(Self::Renewing),
            "failed" => Ok(Self::Failed),
            "rate_limited" => Ok(Self::RateLimited),
            "inactive" => Ok(Self::Inactive),
            _ => Err(FieldError::new("state", "unknown ACME state")),
        }
    }
}

/// The per-subsystem ACME status row (K11). Coordination state, not `Config`:
/// it is written without a revision and read by every instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeStatus {
    pub subsystem_id: SubsystemId,
    pub state: AcmeState,
    /// The names the next order covers.
    pub eligible_names: Vec<String>,
    pub skipped_names: Vec<SkippedName>,
    /// The ACME certificate attached, if one was issued.
    pub certificate_id: Option<CertificateId>,
    pub next_attempt_at_ms: Option<i64>,
    /// Failed orders since the last success (drives the backoff).
    pub failures: u32,
    /// Failed orders since `failure_window_start_ms` (drives the hourly cap).
    pub window_failures: u32,
    pub failure_window_start_ms: Option<i64>,
    /// A sanitized, single-line detail of the last failure; empty after a success.
    pub last_error: String,
    pub last_attempt_at_ms: Option<i64>,
    pub last_success_at_ms: Option<i64>,
    /// The instance that last wrote the row.
    pub instance: String,
}

impl AcmeStatus {
    /// A fresh `idle` status for `subsystem_id`, before anything was attempted.
    pub fn new(subsystem_id: SubsystemId) -> Self {
        Self {
            subsystem_id,
            state: AcmeState::Idle,
            eligible_names: Vec::new(),
            skipped_names: Vec::new(),
            certificate_id: None,
            next_attempt_at_ms: None,
            failures: 0,
            window_failures: 0,
            failure_window_start_ms: None,
            last_error: String::new(),
            last_attempt_at_ms: None,
            last_success_at_ms: None,
            instance: String::new(),
        }
    }
}

/// Makes an error detail safe to store and show: one line, printable
/// characters only, at most [`MAX_ERROR_DETAIL_CHARS`].
pub fn sanitize_error_detail(raw: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = true;
    for ch in raw.chars() {
        let ch = if ch.is_control() || ch.is_whitespace() {
            ' '
        } else {
            ch
        };
        if ch == ' ' && last_was_space {
            continue;
        }
        last_was_space = ch == ' ';
        if out.chars().count() == MAX_ERROR_DETAIL_CHARS {
            out.pop();
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.trim_end().to_string()
}

/// An HTTP-01 challenge token: base64url, 1 to 128 characters. Validated so it
/// can be compared against a request path and stored as a key.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AcmeToken(String);

impl AcmeToken {
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        if raw.is_empty() || raw.len() > MAX_TOKEN_LEN || !is_base64url(raw) {
            return Err(FieldError::new(
                "token",
                "must be 1 to 128 base64url characters",
            ));
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AcmeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AcmeToken({})", self.0)
    }
}

/// A pending HTTP-01 challenge, answered by every instance from the snapshot.
/// The key authorization is public by design (the CA fetches it over plain
/// HTTP), so it is not a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeChallenge {
    pub token: AcmeToken,
    pub subsystem_id: SubsystemId,
    /// The hosts this token is answered for (lowercase).
    pub names: Vec<String>,
    pub key_authorization: String,
    pub expires_at_ms: i64,
}

impl AcmeChallenge {
    /// Builds a challenge, checking the key authorization is `token.thumbprint`
    /// and every name is a valid exact DNS name.
    pub fn new(
        token: AcmeToken,
        subsystem_id: SubsystemId,
        names: Vec<String>,
        key_authorization: String,
        expires_at_ms: i64,
    ) -> Result<Self, FieldError> {
        let thumbprint = key_authorization
            .strip_prefix(token.as_str())
            .and_then(|rest| rest.strip_prefix('.'));
        let well_formed = key_authorization.len() <= MAX_KEY_AUTHORIZATION_LEN
            && thumbprint.is_some_and(|t| !t.is_empty() && is_base64url(t));
        if !well_formed {
            return Err(FieldError::new(
                "keyAuthorization",
                "must be the token, a dot and a base64url thumbprint",
            ));
        }
        if names.is_empty() {
            return Err(FieldError::new("names", "must list at least one name"));
        }
        for name in &names {
            match HostPattern::parse(name) {
                Ok(HostPattern::Exact(exact)) if exact == *name => {}
                _ => {
                    return Err(FieldError::new(
                        "names",
                        format!("{name:?} is not an exact lowercase name"),
                    ))
                }
            }
        }
        Ok(Self {
            token,
            subsystem_id,
            names,
            key_authorization,
            expires_at_ms,
        })
    }

    /// Whether this challenge answers for `host` (lowercase, no port).
    pub fn answers_for(&self, host: &str) -> bool {
        self.names.iter().any(|name| name == host)
    }
}

/// Who may talk to the CA, and until when (K10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeLease {
    pub holder: String,
    pub expires_at_ms: i64,
}

fn is_base64url(raw: &str) -> bool {
    raw.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(entries: &[&str]) -> DomainAllowlist {
        DomainAllowlist::parse(&entries.iter().map(|e| e.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn allowlist_matches_exact_names_and_any_depth_below_a_wildcard() {
        let list = allowlist(&["example.org", "*.Example.com"]);
        assert!(list.allows("example.org"));
        assert!(!list.allows("www.example.org"));
        assert!(list.allows("shop.example.com"));
        assert!(list.allows("a.b.example.com"));
        assert!(!list.allows("example.com"));
        assert!(!list.allows("badexample.com"));
        assert!(!list.allows("example.com.evil.net"));
        assert_eq!(list.patterns(), ["example.org", "*.example.com"]);
    }

    #[test]
    fn allowlist_refuses_empty_too_long_and_invalid_lists() {
        assert_eq!(
            DomainAllowlist::parse(&[]).unwrap_err().field,
            "allowed_domains"
        );
        let many = vec!["a.com".to_string(); MAX_ALLOWED_DOMAINS + 1];
        assert_eq!(
            DomainAllowlist::parse(&many).unwrap_err().field,
            "allowed_domains"
        );
        let err = DomainAllowlist::parse(&["exa mple.com".to_string()]).unwrap_err();
        assert_eq!(err.field, "allowed_domains");
        assert!(err.message.contains("exa mple.com"));
    }

    #[test]
    fn enums_round_trip_and_reject_unknown_spellings() {
        for key in [AcmeKeyType::EcdsaP256, AcmeKeyType::EcdsaP384] {
            assert_eq!(AcmeKeyType::parse(key.as_str()).unwrap(), key);
        }
        assert!(AcmeKeyType::parse("rsa-2048").is_err());
        for reason in [
            SkipReason::WildcardNeedsDns01,
            SkipReason::SingleLabel,
            SkipReason::NotAllowed,
            SkipReason::TooManyNames,
        ] {
            assert_eq!(SkipReason::parse(reason.as_str()).unwrap(), reason);
        }
        assert!(SkipReason::parse("x").is_err());
        for state in [
            AcmeState::Idle,
            AcmeState::Pending,
            AcmeState::Valid,
            AcmeState::Renewing,
            AcmeState::Failed,
            AcmeState::RateLimited,
            AcmeState::Inactive,
        ] {
            assert_eq!(AcmeState::parse(state.as_str()).unwrap(), state);
        }
        assert!(AcmeState::parse("ok").is_err());
    }

    #[test]
    fn tokens_are_base64url_and_bounded() {
        assert!(AcmeToken::parse("abc-DEF_123").is_ok());
        for bad in [
            "",
            "a/b",
            "a.b",
            "a b",
            "a=",
            &"a".repeat(MAX_TOKEN_LEN + 1),
        ] {
            assert_eq!(AcmeToken::parse(bad).unwrap_err().field, "token", "{bad:?}");
        }
    }

    fn challenge(key_authorization: &str, names: &[&str]) -> Result<AcmeChallenge, FieldError> {
        AcmeChallenge::new(
            AcmeToken::parse("tok").unwrap(),
            SubsystemId(1),
            names.iter().map(|n| n.to_string()).collect(),
            key_authorization.to_string(),
            1_000,
        )
    }

    #[test]
    fn a_challenge_needs_a_matching_key_authorization() {
        let ok = challenge("tok.thumb_print-1", &["shop.example.com"]).unwrap();
        assert!(ok.answers_for("shop.example.com"));
        assert!(!ok.answers_for("other.example.com"));
        for bad in ["tok", "tok.", "other.thumb", "tok.th/umb", "tokthumb"] {
            assert_eq!(
                challenge(bad, &["a.com"]).unwrap_err().field,
                "keyAuthorization",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_challenge_names_exact_lowercase_hosts() {
        for bad in [&[][..], &["*.a.com"], &["A.com"], &["a b"]] {
            assert_eq!(
                challenge("tok.t", bad).unwrap_err().field,
                "names",
                "{bad:?}"
            );
        }
    }

    #[test]
    fn error_details_become_one_bounded_line() {
        assert_eq!(sanitize_error_detail("  a\n\tb \u{7}c  "), "a b c");
        let long = sanitize_error_detail(&"x".repeat(1_000));
        assert_eq!(long.chars().count(), MAX_ERROR_DETAIL_CHARS);
        assert!(long.ends_with('…'));
        assert_eq!(sanitize_error_detail(""), "");
    }
}
