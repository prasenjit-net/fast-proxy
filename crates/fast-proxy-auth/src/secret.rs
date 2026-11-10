use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fast_proxy_model::SecretHash;
use ring::digest::{digest, SHA256};
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;
use thiserror::Error;

/// Random bytes in a secret: 256 bits.
const SECRET_BYTES: usize = 32;

/// The system's random source failed; nothing sensible can be generated.
#[derive(Debug, Error)]
#[error("the system random number generator failed")]
pub struct SecretError;

/// A fresh random secret and its hash. The plaintext is shown to the operator
/// once; only the hash is stored.
pub struct GeneratedSecret {
    value: String,
    hash: SecretHash,
}

impl GeneratedSecret {
    /// 256 random bits, base64url (43 characters), with its hash.
    pub fn generate() -> Result<Self, SecretError> {
        let mut bytes = [0u8; SECRET_BYTES];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| SecretError)?;
        let value = URL_SAFE_NO_PAD.encode(bytes);
        let hash = hash_secret(&value);
        Ok(Self { value, hash })
    }

    /// The plaintext secret.
    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn hash(&self) -> &SecretHash {
        &self.hash
    }
}

impl std::fmt::Debug for GeneratedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedSecret").finish_non_exhaustive()
    }
}

/// SHA-256 of a secret. A fast hash is right here: the secret is 256 random
/// bits, so there is nothing to brute-force.
pub fn hash_secret(secret: &str) -> SecretHash {
    let digest = digest(&SHA256, secret.as_bytes());
    let mut bytes = [0u8; SecretHash::LEN];
    bytes.copy_from_slice(digest.as_ref());
    SecretHash::new(bytes)
}

/// Whether `presented` is the secret `stored` was made from, compared in
/// constant time (the comparison does not branch on the bytes).
pub fn secret_matches(presented: &str, stored: &SecretHash) -> bool {
    let presented = hash_secret(presented);
    presented.as_bytes().ct_eq(stored.as_bytes()).into()
}

/// The bypass-header check: an absent header never matches.
pub fn bypass_header_matches(presented: Option<&str>, stored: &SecretHash) -> bool {
    presented.is_some_and(|value| secret_matches(value, stored))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_secret_is_43_url_safe_characters_and_unique() {
        let a = GeneratedSecret::generate().unwrap();
        let b = GeneratedSecret::generate().unwrap();
        assert_eq!(a.value().len(), 43);
        assert!(a
            .value()
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'));
        assert_ne!(a.value(), b.value());
    }

    #[test]
    fn the_stored_form_is_a_hash_not_the_secret() {
        let secret = GeneratedSecret::generate().unwrap();
        assert_eq!(secret.hash().as_bytes().len(), 32);
        assert_ne!(secret.hash().as_bytes(), secret.value().as_bytes());
        assert_eq!(*secret.hash(), hash_secret(secret.value()));
    }

    #[test]
    fn the_hash_is_sha256() {
        // SHA-256("abc"), FIPS 180-2 test vector.
        let expected = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(hash_secret("abc").as_bytes(), expected);
    }

    #[test]
    fn matching_accepts_the_secret_and_rejects_everything_else() {
        let secret = GeneratedSecret::generate().unwrap();
        assert!(secret_matches(secret.value(), secret.hash()));
        let truncated = &secret.value()[..42];
        let altered = format!("{}x", &secret.value()[..42]);
        for wrong in ["", "x", truncated, altered.as_str()] {
            assert!(!secret_matches(wrong, secret.hash()), "{wrong:?}");
        }
    }

    #[test]
    fn an_absent_header_never_matches() {
        let secret = GeneratedSecret::generate().unwrap();
        assert!(bypass_header_matches(Some(secret.value()), secret.hash()));
        assert!(!bypass_header_matches(Some("nope"), secret.hash()));
        assert!(!bypass_header_matches(None, secret.hash()));
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let secret = GeneratedSecret::generate().unwrap();
        assert!(!format!("{secret:?}").contains(secret.value()));
    }
}
