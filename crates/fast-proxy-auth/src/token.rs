use fast_proxy_model::TokenId;
use thiserror::Error;

use crate::secret::GeneratedSecret;

const PREFIX: &str = "fp_";

/// Why a presented token could not even be looked up.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TokenError {
    #[error("not an admin token")]
    Malformed,
}

/// A complete token, `fp_<id>.<secret>`, as shown to the operator once.
/// `Debug` hides it.
pub struct PlaintextToken(String);

impl PlaintextToken {
    /// Builds the token text once the database has assigned the id.
    pub fn new(id: TokenId, secret: &GeneratedSecret) -> Self {
        Self(format!("{PREFIX}{}.{}", id.0, secret.value()))
    }

    /// The text to give the operator.
    pub fn reveal(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for PlaintextToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlaintextToken(..)")
    }
}

/// A presented token split into its id (a lookup key) and secret.
pub struct ParsedToken<'a> {
    pub id: TokenId,
    pub secret: &'a str,
}

impl<'a> ParsedToken<'a> {
    /// Splits `fp_<id>.<secret>`; anything else is `Malformed`.
    pub fn parse(raw: &'a str) -> Result<Self, TokenError> {
        let rest = raw.strip_prefix(PREFIX).ok_or(TokenError::Malformed)?;
        let (id, secret) = rest.split_once('.').ok_or(TokenError::Malformed)?;
        let well_formed_id = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
        if !well_formed_id || secret.is_empty() {
            return Err(TokenError::Malformed);
        }
        let id = id.parse::<i64>().map_err(|_| TokenError::Malformed)?;
        Ok(Self {
            id: TokenId(id),
            secret,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_round_trips_through_text() {
        let secret = GeneratedSecret::generate().unwrap();
        let token = PlaintextToken::new(TokenId(42), &secret);
        assert!(token.reveal().starts_with("fp_42."));
        let parsed = ParsedToken::parse(token.reveal()).unwrap();
        assert_eq!(parsed.id, TokenId(42));
        assert_eq!(parsed.secret, secret.value());
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        for raw in [
            "",
            "fp_",
            "fp_.",
            "fp_1",
            "fp_1.",
            "fp_.secret",
            "fp_x.secret",
            "fp_-1.secret",
            "fp_+1.secret",
            "fp_ 1.secret",
            "FP_1.secret",
            "1.secret",
            "Bearer fp_1.secret",
            "fp_99999999999999999999.secret",
        ] {
            assert_eq!(
                ParsedToken::parse(raw).err(),
                Some(TokenError::Malformed),
                "{raw:?}"
            );
        }
    }

    #[test]
    fn a_secret_may_contain_dots_after_the_first() {
        let parsed = ParsedToken::parse("fp_7.a.b").unwrap();
        assert_eq!((parsed.id, parsed.secret), (TokenId(7), "a.b"));
    }

    #[test]
    fn debug_output_hides_the_token() {
        let secret = GeneratedSecret::generate().unwrap();
        let token = PlaintextToken::new(TokenId(1), &secret);
        assert!(!format!("{token:?}").contains(secret.value()));
    }
}
