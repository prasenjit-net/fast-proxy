use std::fmt;

/// The SHA-256 of a secret (a token secret or a maintenance bypass secret).
/// The secret itself is never stored; hashing and comparing live in the auth
/// crate, so this type only carries the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretHash([u8; SecretHash::LEN]);

impl SecretHash {
    /// Length of a SHA-256 digest in bytes.
    pub const LEN: usize = 32;

    pub fn new(bytes: [u8; Self::LEN]) -> Self {
        Self(bytes)
    }

    /// Reads a stored hash back; `None` unless it is exactly 32 bytes.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        <[u8; Self::LEN]>::try_from(bytes).ok().map(Self)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretHash(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_full_digest_is_accepted() {
        assert!(SecretHash::from_slice(&[7; 32]).is_some());
        for length in [0, 31, 33] {
            assert!(SecretHash::from_slice(&vec![7; length]).is_none());
        }
    }

    #[test]
    fn debug_output_hides_the_bytes() {
        assert_eq!(format!("{:?}", SecretHash::new([9; 32])), "SecretHash(..)");
    }
}
