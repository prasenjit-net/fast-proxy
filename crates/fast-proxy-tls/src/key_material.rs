use std::sync::Arc;

use rustls::pki_types::{PrivateKeyDer, PrivatePkcs1KeyDer, PrivatePkcs8KeyDer, PrivateSec1KeyDer};
use rustls::sign::SigningKey;
use zeroize::Zeroizing;

/// The container format of a private key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    Pkcs8,
    Pkcs1,
    Sec1,
}

impl KeyKind {
    fn tag(self) -> u8 {
        match self {
            Self::Pkcs8 => 1,
            Self::Pkcs1 => 2,
            Self::Sec1 => 3,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::Pkcs8),
            2 => Some(Self::Pkcs1),
            3 => Some(Self::Sec1),
            _ => None,
        }
    }
}

/// A private key's DER bytes, zeroed when dropped. It has no `Debug` output
/// beyond its kind, so a stray `{:?}` cannot leak it.
pub struct KeyMaterial {
    kind: KeyKind,
    der: Zeroizing<Vec<u8>>,
}

impl std::fmt::Debug for KeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyMaterial")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl KeyMaterial {
    pub(crate) fn new(kind: KeyKind, der: Vec<u8>) -> Self {
        Self {
            kind,
            der: Zeroizing::new(der),
        }
    }

    pub fn kind(&self) -> KeyKind {
        self.kind
    }

    /// The bytes that get sealed: a kind tag followed by the DER.
    pub fn to_sealed_bytes(&self) -> Zeroizing<Vec<u8>> {
        let mut bytes = Vec::with_capacity(self.der.len() + 1);
        bytes.push(self.kind.tag());
        bytes.extend_from_slice(&self.der);
        Zeroizing::new(bytes)
    }

    /// The inverse of [`to_sealed_bytes`](Self::to_sealed_bytes); `None` for
    /// bytes that are not one.
    pub fn from_sealed_bytes(bytes: &[u8]) -> Option<Self> {
        let (tag, der) = bytes.split_first()?;
        if der.is_empty() {
            return None;
        }
        Some(Self::new(KeyKind::from_tag(*tag)?, der.to_vec()))
    }

    /// The key in rustls's type (a copy of the DER, owned by the caller).
    pub fn to_private_key_der(&self) -> PrivateKeyDer<'static> {
        let der = self.der.to_vec();
        match self.kind {
            KeyKind::Pkcs8 => PrivatePkcs8KeyDer::from(der).into(),
            KeyKind::Pkcs1 => PrivatePkcs1KeyDer::from(der).into(),
            KeyKind::Sec1 => PrivateSec1KeyDer::from(der).into(),
        }
    }

    /// A rustls signing key, or `None` when the algorithm is unsupported.
    pub fn signing_key(&self) -> Option<Arc<dyn SigningKey>> {
        rustls::crypto::ring::sign::any_supported_type(&self.to_private_key_der()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_bytes_round_trip_every_kind() {
        for kind in [KeyKind::Pkcs8, KeyKind::Pkcs1, KeyKind::Sec1] {
            let key = KeyMaterial::new(kind, vec![1, 2, 3]);
            let back = KeyMaterial::from_sealed_bytes(&key.to_sealed_bytes()).unwrap();
            assert_eq!(back.kind(), kind);
            assert_eq!(&*back.der, &[1, 2, 3]);
        }
    }

    #[test]
    fn malformed_sealed_bytes_are_rejected() {
        for bytes in [&[][..], &[1][..], &[9, 1, 2][..]] {
            assert!(KeyMaterial::from_sealed_bytes(bytes).is_none());
        }
    }

    #[test]
    fn debug_output_never_contains_key_bytes() {
        let key = KeyMaterial::new(KeyKind::Pkcs8, vec![0xAB, 0xCD, 0xEF, 0x12]);
        let text = format!("{key:?}");
        assert!(!text.to_lowercase().contains("ab"), "{text}");
        assert!(text.contains("Pkcs8"));
    }

    #[test]
    fn an_unsupported_key_has_no_signing_key() {
        assert!(KeyMaterial::new(KeyKind::Pkcs8, vec![1, 2, 3])
            .signing_key()
            .is_none());
    }
}
