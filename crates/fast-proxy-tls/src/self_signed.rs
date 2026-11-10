//! Self-signed certificates: development certificates for subsystems that
//! enable HTTPS without uploading one (ADR 0010), and the in-memory "unknown
//! host" placeholder that is never stored.

use std::sync::Arc;

use rcgen::{CertificateParams, DnType, KeyPair, SanType};
use rustls::pki_types::CertificateDer;
use rustls::sign::CertifiedKey;
use time::{Duration, OffsetDateTime};

use crate::key_material::{KeyKind, KeyMaterial};

const VALID_DAYS: i64 = 365;
const UNKNOWN_HOST_NAME: &str = "fast-proxy.invalid";

/// A generated certificate, ready to be sealed and stored.
#[derive(Debug)]
pub struct GeneratedCertificate {
    pub chain_pem: String,
    pub key: KeyMaterial,
}

/// Generates a self-signed certificate valid for `names` (exact names and
/// `*.suffix` wildcards), from one day before `now_ms` for a year.
pub fn generate_self_signed(names: &[String], now_ms: i64) -> Result<GeneratedCertificate, String> {
    let first = names.first().ok_or("at least one name is required")?;
    let now = OffsetDateTime::from_unix_timestamp(now_ms.div_euclid(1000))
        .map_err(|err| err.to_string())?;
    let mut params = CertificateParams::default();
    params
        .distinguished_name
        .push(DnType::CommonName, first.as_str());
    params.subject_alt_names = names
        .iter()
        .map(|name| {
            name.as_str()
                .try_into()
                .map(SanType::DnsName)
                .map_err(|_| "a name cannot be used in a certificate".to_string())
        })
        .collect::<Result<_, _>>()?;
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(VALID_DAYS);
    let key = KeyPair::generate().map_err(|err| err.to_string())?;
    let certificate = params.self_signed(&key).map_err(|err| err.to_string())?;
    Ok(GeneratedCertificate {
        chain_pem: certificate.pem(),
        key: KeyMaterial::new(KeyKind::Pkcs8, key.serialize_der()),
    })
}

/// The placeholder certificate served for an unknown or missing SNI. It lives
/// only in memory.
pub fn ephemeral_unknown_host(now_ms: i64) -> Result<Arc<CertifiedKey>, String> {
    let generated = generate_self_signed(&[UNKNOWN_HOST_NAME.to_string()], now_ms)?;
    let chain: Vec<CertificateDer<'static>> =
        crate::validate::parse_chain(&generated.chain_pem).map_err(|err| err.to_string())?;
    let signing_key = generated
        .key
        .signing_key()
        .ok_or("the generated key is unusable")?;
    Ok(Arc::new(CertifiedKey::new(chain, signing_key)))
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{covers, HostPattern};

    use super::*;
    use crate::validate::validate_upload;

    const NOW: i64 = 1_800_000_000_000;

    fn pem_of(generated: &GeneratedCertificate) -> String {
        // The key is exposed only to feed the validator in this test.
        let der = generated.key.to_private_key_der();
        let rustls::pki_types::PrivateKeyDer::Pkcs8(der) = der else {
            panic!("generated keys are PKCS#8");
        };
        use base64::Engine;
        let body = base64::engine::general_purpose::STANDARD.encode(der.secret_pkcs8_der());
        format!("-----BEGIN PRIVATE KEY-----\n{body}\n-----END PRIVATE KEY-----\n")
    }

    #[test]
    fn the_certificate_covers_exact_and_wildcard_names_and_passes_validation() {
        let names = vec![
            "shop.example.com".to_string(),
            "*.wild.example.com".to_string(),
        ];
        let generated = generate_self_signed(&names, NOW).unwrap();
        let validated = validate_upload(&generated.chain_pem, &pem_of(&generated), NOW).unwrap();
        assert_eq!(validated.metadata.names, names);
        assert!(validated
            .metadata
            .covers(&HostPattern::parse("shop.example.com").unwrap()));
        assert!(covers(
            &validated.metadata.names[1],
            &HostPattern::parse("a.wild.example.com").unwrap()
        ));
        assert!(
            validated.metadata.issuer == validated.metadata.subject,
            "self-signed"
        );
    }

    #[test]
    fn validity_is_a_year_starting_a_day_ago() {
        let generated = generate_self_signed(&["a.example.com".into()], NOW).unwrap();
        let validated = validate_upload(&generated.chain_pem, &pem_of(&generated), NOW).unwrap();
        let day = 86_400_000;
        assert_eq!(validated.metadata.not_before_ms, NOW - day);
        assert_eq!(validated.metadata.not_after_ms, NOW + 365 * day);
    }

    #[test]
    fn no_names_or_an_unusable_name_is_an_error() {
        assert!(generate_self_signed(&[], NOW).is_err());
        assert!(generate_self_signed(&["not a name \u{1F600}".into()], NOW).is_err());
    }

    #[test]
    fn the_ephemeral_certificate_is_in_memory_with_a_reserved_name() {
        let certified = ephemeral_unknown_host(NOW).unwrap();
        assert_eq!(certified.cert.len(), 1);
        let metadata = crate::validate::read_metadata(&certified.cert[0]).unwrap();
        assert_eq!(metadata.names, ["fast-proxy.invalid"]);
        assert!(certified.keys_match().is_ok());
    }
}
