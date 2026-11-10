//! Parsing and checking certificate PEM supplied by an admin. Errors name the
//! field (`certPem`, `keyPem`) and never contain PEM text.

use fast_proxy_model::{
    validate_san, CertificateMetadata, FieldError, MAX_CHAIN_LENGTH, MAX_PEM_BYTES,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::key_material::{KeyKind, KeyMaterial};
use crate::status::EXPIRING_SOON_DAYS;

const MS_PER_DAY: i64 = 86_400_000;

/// A certificate and key that passed every upload check.
#[derive(Debug)]
pub struct ValidatedCertificate {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: KeyMaterial,
    pub metadata: CertificateMetadata,
    /// Non-fatal findings, such as a certificate expiring soon.
    pub warnings: Vec<String>,
}

/// How close a certificate is to the end of its validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryState {
    Ok,
    /// Under [`EXPIRING_SOON_DAYS`] left.
    Expiring,
    Expired,
}

/// Classifies `not_after_ms` against `now_ms`.
pub fn expiry_state(not_after_ms: i64, now_ms: i64) -> ExpiryState {
    if not_after_ms <= now_ms {
        ExpiryState::Expired
    } else if not_after_ms - now_ms < EXPIRING_SOON_DAYS * MS_PER_DAY {
        ExpiryState::Expiring
    } else {
        ExpiryState::Ok
    }
}

/// Checks an uploaded chain and key: both parse, the key matches the leaf, the
/// leaf has a usable DNS name and is within its validity at `now_ms`.
pub fn validate_upload(
    chain_pem: &str,
    key_pem: &str,
    now_ms: i64,
) -> Result<ValidatedCertificate, FieldError> {
    let chain = parse_chain(chain_pem)?;
    let key = parse_key(key_pem)?;
    let leaf = chain
        .first()
        .ok_or_else(|| FieldError::new("certPem", "contains no certificate"))?;
    let metadata = read_metadata(leaf)?;
    check_key_matches(&chain, &key)?;
    if metadata.not_before_ms > now_ms {
        return Err(FieldError::new("certPem", "is not valid yet"));
    }
    let mut warnings = Vec::new();
    match expiry_state(metadata.not_after_ms, now_ms) {
        ExpiryState::Expired => return Err(FieldError::new("certPem", "has expired")),
        ExpiryState::Expiring => warnings.push(format!(
            "the certificate expires in under {EXPIRING_SOON_DAYS} days"
        )),
        ExpiryState::Ok => {}
    }
    Ok(ValidatedCertificate {
        chain,
        key,
        metadata,
        warnings,
    })
}

/// Parses a PEM certificate chain, leaf first.
pub fn parse_chain(chain_pem: &str) -> Result<Vec<CertificateDer<'static>>, FieldError> {
    if chain_pem.len() > MAX_PEM_BYTES {
        return Err(FieldError::new("certPem", "must be at most 256 KiB"));
    }
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| FieldError::new("certPem", "is not valid PEM certificate data"))?;
    if chain.is_empty() {
        return Err(FieldError::new(
            "certPem",
            "contains no PEM certificate (expected BEGIN CERTIFICATE)",
        ));
    }
    if chain.len() > MAX_CHAIN_LENGTH {
        return Err(FieldError::new("certPem", "has more than 10 certificates"));
    }
    Ok(chain)
}

/// Parses an unencrypted PEM private key (PKCS#8, PKCS#1 or SEC1).
pub fn parse_key(key_pem: &str) -> Result<KeyMaterial, FieldError> {
    if key_pem.len() > MAX_PEM_BYTES {
        return Err(FieldError::new("keyPem", "must be at most 256 KiB"));
    }
    if is_encrypted(key_pem) {
        return Err(FieldError::new(
            "keyPem",
            "is passphrase-protected; upload an unencrypted key (the proxy encrypts it at rest)",
        ));
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| {
        FieldError::new(
            "keyPem",
            "contains no PEM private key (expected BEGIN PRIVATE KEY)",
        )
    })?;
    let (kind, der) = match &key {
        PrivateKeyDer::Pkcs8(der) => (KeyKind::Pkcs8, der.secret_pkcs8_der().to_vec()),
        PrivateKeyDer::Pkcs1(der) => (KeyKind::Pkcs1, der.secret_pkcs1_der().to_vec()),
        PrivateKeyDer::Sec1(der) => (KeyKind::Sec1, der.secret_sec1_der().to_vec()),
        _ => return Err(FieldError::new("keyPem", "has an unsupported key format")),
    };
    Ok(KeyMaterial::new(kind, der))
}

/// Only header lines are inspected, so key bytes can never trigger it.
fn is_encrypted(key_pem: &str) -> bool {
    key_pem.lines().any(|line| {
        line.starts_with("-----BEGIN ENCRYPTED")
            || (line.starts_with("Proc-Type:") && line.contains("ENCRYPTED"))
    })
}

fn check_key_matches(
    chain: &[CertificateDer<'static>],
    key: &KeyMaterial,
) -> Result<(), FieldError> {
    let signing_key = key
        .signing_key()
        .ok_or_else(|| FieldError::new("keyPem", "uses an unsupported key type or is corrupt"))?;
    CertifiedKey::new(chain.to_vec(), signing_key)
        .keys_match()
        .map_err(|_| FieldError::new("keyPem", "does not match the certificate"))
}

/// Extracts names, subject, issuer, validity and fingerprint from a leaf.
/// Fails when the first certificate is a CA (the chain is in the wrong order)
/// or has no usable DNS name.
pub fn read_metadata(leaf: &CertificateDer<'_>) -> Result<CertificateMetadata, FieldError> {
    let (_, cert) = X509Certificate::from_der(leaf.as_ref())
        .map_err(|_| FieldError::new("certPem", "contains a certificate that cannot be parsed"))?;
    if cert.is_ca() {
        return Err(FieldError::new(
            "certPem",
            "starts with a CA certificate; put the server certificate first, then its chain",
        ));
    }
    let names = dns_names(&cert);
    if names.is_empty() {
        return Err(FieldError::new(
            "certPem",
            "has no usable DNS name in its subject alternative names",
        ));
    }
    Ok(CertificateMetadata {
        names,
        subject: cert.subject().to_string(),
        issuer: cert.issuer().to_string(),
        not_before_ms: cert.validity().not_before.timestamp().saturating_mul(1000),
        not_after_ms: cert.validity().not_after.timestamp().saturating_mul(1000),
        fingerprint: hex(&Sha256::digest(leaf.as_ref())),
    })
}

fn dns_names(cert: &X509Certificate<'_>) -> Vec<String> {
    let Ok(Some(san)) = cert.subject_alternative_name() else {
        return Vec::new();
    };
    let mut names: Vec<String> = san
        .value
        .general_names
        .iter()
        .filter_map(|name| match name {
            GeneralName::DNSName(dns) => Some(dns.to_ascii_lowercase()),
            _ => None,
        })
        .filter(|dns| validate_san(dns).is_ok())
        .collect();
    names.dedup();
    names
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use fast_proxy_test_pki::{pkcs8_pem_to_sec1_pem, LeafSpec, TestCa};

    use super::*;

    fn now_ms() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    }

    #[test]
    fn a_good_certificate_and_key_pass_with_their_metadata() {
        let ca = TestCa::new("test ca");
        let issued = ca.issue(&LeafSpec::new(&["Shop.Example.com", "*.wild.example.com"]));
        let validated = validate_upload(&issued.chain_pem, &issued.key_pem, now_ms()).unwrap();
        assert_eq!(validated.chain.len(), 2);
        assert_eq!(
            validated.metadata.names,
            ["shop.example.com", "*.wild.example.com"]
        );
        assert!(validated.metadata.subject.contains("CN="));
        assert!(validated.metadata.issuer.contains("test ca"));
        assert_eq!(validated.metadata.fingerprint.len(), 64);
        assert!(validated.warnings.is_empty());
        assert_eq!(validated.key.kind(), KeyKind::Pkcs8);
    }

    #[test]
    fn a_sec1_key_is_accepted_and_matched_to_its_certificate() {
        let issued = TestCa::new("ca").issue(&LeafSpec::new(&["ec.example.com"]));
        let sec1 = pkcs8_pem_to_sec1_pem(&issued.key_pem);
        assert!(sec1.contains("BEGIN EC PRIVATE KEY"));
        let validated = validate_upload(&issued.cert_pem, &sec1, now_ms()).unwrap();
        assert_eq!(validated.key.kind(), KeyKind::Sec1);
    }

    #[test]
    fn a_pkcs1_pem_is_recognized_by_its_kind() {
        // The bytes are not a real RSA key: this checks only the format dispatch.
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert_eq!(parse_key(pem).unwrap().kind(), KeyKind::Pkcs1);
        let issued = TestCa::new("ca").issue(&LeafSpec::new(&["a.example.com"]));
        let err = validate_upload(&issued.cert_pem, pem, now_ms()).unwrap_err();
        assert_eq!(err.field, "keyPem");
    }

    #[test]
    fn an_expired_certificate_is_rejected_and_a_soon_expiring_one_warns() {
        let ca = TestCa::new("ca");
        let expired = ca.issue(&LeafSpec::new(&["a.example.com"]).expiring_in_days(-2));
        let err = validate_upload(&expired.cert_pem, &expired.key_pem, now_ms()).unwrap_err();
        assert_eq!(
            (err.field.as_str(), err.message.as_str()),
            ("certPem", "has expired")
        );

        let soon = ca.issue(&LeafSpec::new(&["a.example.com"]).expiring_in_days(5));
        let validated = validate_upload(&soon.cert_pem, &soon.key_pem, now_ms()).unwrap();
        assert_eq!(validated.warnings.len(), 1);
    }

    #[test]
    fn a_not_yet_valid_certificate_is_rejected() {
        let issued =
            TestCa::new("ca").issue(&LeafSpec::new(&["a.example.com"]).starting_in_days(3));
        let err = validate_upload(&issued.cert_pem, &issued.key_pem, now_ms()).unwrap_err();
        assert_eq!(err.field, "certPem");
        assert!(err.message.contains("not valid yet"));
    }

    #[test]
    fn a_key_that_does_not_match_is_rejected() {
        let ca = TestCa::new("ca");
        let one = ca.issue(&LeafSpec::new(&["a.example.com"]));
        let other = ca.issue(&LeafSpec::new(&["b.example.com"]));
        let err = validate_upload(&one.cert_pem, &other.key_pem, now_ms()).unwrap_err();
        assert_eq!(err.field, "keyPem");
        assert!(err.message.contains("does not match"));
    }

    #[test]
    fn garbage_and_empty_input_are_rejected_without_echoing_it() {
        let ca = TestCa::new("ca");
        let good = ca.issue(&LeafSpec::new(&["a.example.com"]));
        for bad in [
            "",
            "   ",
            "hello world SECRET-TEXT",
            "-----BEGIN CERTIFICATE-----\nSECRET!!\n-----END CERTIFICATE-----\n",
        ] {
            let err = validate_upload(bad, &good.key_pem, now_ms()).unwrap_err();
            assert_eq!(err.field, "certPem", "{bad}");
            assert!(!err.to_string().contains("SECRET"), "{err}");
        }
        for bad in [
            "",
            "not a key SECRET-TEXT",
            "-----BEGIN PRIVATE KEY-----\nSECRET\n-----END PRIVATE KEY-----\n",
        ] {
            let err = validate_upload(&good.cert_pem, bad, now_ms()).unwrap_err();
            assert_eq!(err.field, "keyPem", "{bad}");
            assert!(!err.to_string().contains("SECRET"), "{err}");
        }
    }

    #[test]
    fn an_encrypted_key_is_rejected_with_a_clear_message() {
        let good = TestCa::new("ca").issue(&LeafSpec::new(&["a.example.com"]));
        let encrypted =
            "-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n";
        let err = validate_upload(&good.cert_pem, encrypted, now_ms()).unwrap_err();
        assert_eq!(err.field, "keyPem");
        assert!(err.message.contains("passphrase"));
        let legacy = "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n";
        assert!(parse_key(legacy)
            .unwrap_err()
            .message
            .contains("passphrase"));
    }

    #[test]
    fn a_chain_in_the_wrong_order_is_rejected() {
        let issued = TestCa::new("ca").issue(&LeafSpec::new(&["a.example.com"]));
        let reversed = format!("{}{}", issued.ca_pem, issued.cert_pem);
        let err = validate_upload(&reversed, &issued.key_pem, now_ms()).unwrap_err();
        assert_eq!(err.field, "certPem");
        assert!(err.message.contains("CA certificate"), "{err}");
    }

    #[test]
    fn a_certificate_with_no_dns_name_is_rejected() {
        let issued = TestCa::new("ca").issue(&LeafSpec::new(&["10.1.2.3"]));
        let err = validate_upload(&issued.cert_pem, &issued.key_pem, now_ms()).unwrap_err();
        assert!(err.message.contains("DNS name"), "{err}");
    }

    #[test]
    fn size_and_chain_length_limits_apply() {
        let issued = TestCa::new("ca").issue(&LeafSpec::new(&["a.example.com"]));
        assert!(parse_chain(&"x".repeat(MAX_PEM_BYTES + 1)).is_err());
        let long = issued.cert_pem.repeat(MAX_CHAIN_LENGTH + 1);
        assert!(parse_chain(&long)
            .unwrap_err()
            .message
            .contains("more than 10"));
        assert_eq!(
            parse_chain(&issued.cert_pem.repeat(MAX_CHAIN_LENGTH))
                .unwrap()
                .len(),
            10
        );
        assert!(parse_key(&"x".repeat(MAX_PEM_BYTES + 1)).is_err());
    }

    #[test]
    fn expiry_is_classified_at_the_boundaries() {
        let day = MS_PER_DAY;
        assert_eq!(expiry_state(1_000, 1_000), ExpiryState::Expired);
        assert_eq!(expiry_state(1_001, 1_000), ExpiryState::Expiring);
        assert_eq!(expiry_state(14 * day, 0), ExpiryState::Ok);
        assert_eq!(expiry_state(14 * day - 1, 0), ExpiryState::Expiring);
        assert_eq!(expiry_state(-5, 0), ExpiryState::Expired);
    }
}
