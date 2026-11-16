//! The HTTP-01 challenges every instance answers (ADR 0025): a token, the
//! hosts it is for and its key authorization. Answered from the snapshot so
//! the data plane never reads the database; the key authorization is public by
//! design, so nothing here is secret.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{dns_host, AcmeInputs};

/// The path every HTTP-01 challenge is fetched under (RFC 8555 section 8.3).
pub const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Answer {
    names: Vec<String>,
    key_authorization: Arc<str>,
}

/// The live challenges by token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AcmeChallenges {
    by_token: HashMap<String, Answer>,
}

impl AcmeChallenges {
    /// Keeps the challenges that have not expired at `inputs.now_ms`.
    pub fn build(inputs: &AcmeInputs) -> Self {
        let by_token = inputs
            .challenges
            .iter()
            .filter(|challenge| challenge.expires_at_ms > inputs.now_ms)
            .map(|challenge| {
                (
                    challenge.token.as_str().to_string(),
                    Answer {
                        names: challenge.names.clone(),
                        key_authorization: challenge.key_authorization.as_str().into(),
                    },
                )
            })
            .collect();
        Self { by_token }
    }

    /// How many challenges are live.
    pub fn len(&self) -> usize {
        self.by_token.len()
    }

    /// Whether no challenge is live (the common case: the check is then free).
    pub fn is_empty(&self) -> bool {
        self.by_token.is_empty()
    }

    /// The key authorization to answer a request for `path` on `host_header`
    /// with: only for a challenge path whose token is live and was issued for
    /// that host. Anything else is `None`, and the request is routed as usual.
    pub fn answer(&self, host_header: &str, path: &str) -> Option<Arc<str>> {
        if self.by_token.is_empty() {
            return None;
        }
        let token = path.strip_prefix(ACME_CHALLENGE_PREFIX)?;
        let answer = self.by_token.get(token)?;
        let host = dns_host(host_header)?;
        answer
            .names
            .contains(&host)
            .then(|| answer.key_authorization.clone())
    }
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{AcmeChallenge, AcmeToken, SubsystemId};

    use super::*;

    fn challenge(token: &str, names: &[&str], expires_at_ms: i64) -> AcmeChallenge {
        AcmeChallenge::new(
            AcmeToken::parse(token).unwrap(),
            SubsystemId(1),
            names.iter().map(ToString::to_string).collect(),
            format!("{token}.thumb"),
            expires_at_ms,
        )
        .unwrap()
    }

    fn built(challenges: Vec<AcmeChallenge>, now_ms: i64) -> AcmeChallenges {
        AcmeChallenges::build(&AcmeInputs { challenges, now_ms })
    }

    #[test]
    fn a_live_token_is_answered_for_its_hosts_only() {
        let live = built(vec![challenge("tok", &["shop.example.com"], 100)], 0);
        let path = "/.well-known/acme-challenge/tok";
        assert_eq!(
            live.answer("shop.example.com", path).as_deref(),
            Some("tok.thumb")
        );
        assert_eq!(
            live.answer("SHOP.example.com:80", path).as_deref(),
            Some("tok.thumb")
        );
        assert_eq!(
            live.answer("shop.example.com.", path).as_deref(),
            Some("tok.thumb")
        );
        assert_eq!(live.answer("other.example.com", path), None);
        assert_eq!(live.answer("", path), None);
    }

    #[test]
    fn other_paths_and_unknown_tokens_are_not_answered() {
        let live = built(vec![challenge("tok", &["a.example.com"], 100)], 0);
        for path in [
            "/.well-known/acme-challenge/other",
            "/.well-known/acme-challenge/",
            "/.well-known/acme-challenge/tok/extra",
            "/.well-known/acme-challenge",
            "/tok",
            "/x/.well-known/acme-challenge/tok",
        ] {
            assert_eq!(live.answer("a.example.com", path), None, "{path}");
        }
    }

    #[test]
    fn expired_challenges_are_left_out_at_compile() {
        let compiled = built(
            vec![
                challenge("old", &["a.example.com"], 50),
                challenge("new", &["a.example.com"], 51),
            ],
            50,
        );
        assert_eq!(compiled.len(), 1);
        assert!(compiled
            .answer("a.example.com", "/.well-known/acme-challenge/new")
            .is_some());
        assert!(built(Vec::new(), 0).is_empty());
    }
}
