//! What the admin plane and the metrics say about a certificate.

use fast_proxy_telemetry::CertificateReading;

use crate::cache::{CertCache, CertEntry};
use crate::validate::{expiry_state, ExpiryState};

/// Below this many days left a certificate is "expiring".
pub const EXPIRING_SOON_DAYS: i64 = 14;

/// The state shown for a certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificateState {
    Ok,
    Expiring,
    Expired,
    /// Stored, but the key could not be read (for example a wrong master key).
    Unreadable,
}

impl CertificateState {
    /// The API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Expiring => "expiring",
            Self::Expired => "expired",
            Self::Unreadable => "unreadable",
        }
    }

    /// The state for a certificate ending at `not_after_ms`.
    pub fn of(not_after_ms: i64, unreadable: bool, now_ms: i64) -> Self {
        if unreadable {
            return Self::Unreadable;
        }
        match expiry_state(not_after_ms, now_ms) {
            ExpiryState::Ok => Self::Ok,
            ExpiryState::Expiring => Self::Expiring,
            ExpiryState::Expired => Self::Expired,
        }
    }
}

/// One reading per cached certificate, for the expiry and unreadable gauges.
pub fn certificate_readings(cache: &CertCache, now_ms: i64) -> Vec<CertificateReading> {
    cache
        .entries()
        .map(|entry| {
            let (name, source, metadata, unreadable) = match entry {
                CertEntry::Ready(handle) => {
                    (handle.name.as_str(), handle.source, &handle.metadata, false)
                }
                CertEntry::Unreadable(cert) => {
                    (cert.name.as_str(), cert.source, &cert.metadata, true)
                }
            };
            CertificateReading {
                name: name.to_string(),
                source: source.as_str(),
                seconds_left: (metadata.not_after_ms - now_ms).div_euclid(1000),
                unreadable,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_prefers_unreadable_then_expiry() {
        let day = 86_400_000;
        assert_eq!(
            CertificateState::of(100 * day, true, 0),
            CertificateState::Unreadable
        );
        assert_eq!(
            CertificateState::of(100 * day, false, 0),
            CertificateState::Ok
        );
        assert_eq!(
            CertificateState::of(3 * day, false, 0),
            CertificateState::Expiring
        );
        assert_eq!(
            CertificateState::of(-1, false, 0),
            CertificateState::Expired
        );
        for (state, text) in [
            (CertificateState::Ok, "ok"),
            (CertificateState::Expiring, "expiring"),
            (CertificateState::Expired, "expired"),
            (CertificateState::Unreadable, "unreadable"),
        ] {
            assert_eq!(state.as_str(), text);
        }
    }

    #[test]
    fn readings_cover_ready_and_unreadable_certificates() {
        use fast_proxy_model::{CertificateId, CertificateName, CertificateSource};

        use crate::cache::StoredCertificate;
        use crate::self_signed::generate_self_signed;
        use crate::validate::{parse_chain, read_metadata};
        use crate::vault::{MasterKey, Vault};

        let now = 1_800_000_000_000;
        let key = MasterKey::from_base64("AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=").unwrap();
        let vault = Vault::new(&key);
        let rows: Vec<StoredCertificate> = (1..=2)
            .map(|id| {
                let name = format!("c{id}");
                let generated =
                    generate_self_signed(&[format!("{name}.example.com")], now).unwrap();
                let metadata =
                    read_metadata(&parse_chain(&generated.chain_pem).unwrap()[0]).unwrap();
                let sealed_key = vault
                    .seal_key(CertificateId(id), &name, &generated.key.to_sealed_bytes())
                    .unwrap();
                StoredCertificate {
                    id: CertificateId(id),
                    name: CertificateName::parse(&name).unwrap(),
                    source: CertificateSource::SelfSigned,
                    chain_pem: generated.chain_pem,
                    sealed_key: if id == 2 {
                        crate::vault::Sealed {
                            nonce: vec![0; 12],
                            ciphertext: vec![0; 32],
                        }
                    } else {
                        sealed_key
                    },
                    metadata,
                    updated_at_ms: 1,
                }
            })
            .collect();
        let (cache, _) = CertCache::empty().rebuild(&rows, Some(&vault));
        let readings = certificate_readings(&cache, now);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].name, "c1");
        assert!(!readings[0].unreadable);
        assert_eq!(readings[0].seconds_left, 365 * 86_400);
        assert_eq!(readings[0].source, "self_signed");
        assert!(readings[1].unreadable);
    }
}
