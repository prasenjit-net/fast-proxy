//! The key and certificate signing request of one ACME order (M4, K13): a fresh
//! ECDSA key per order, so a renewed certificate never reuses the old key, and
//! a CSR naming exactly the names being ordered.

use fast_proxy_model::AcmeKeyType;
use rcgen::{
    CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384,
};

use crate::key_material::{KeyKind, KeyMaterial};

/// A generated key and the CSR it signed. The key is zeroed on drop and never
/// printed; the CSR is public.
pub struct OrderKey {
    pub key: KeyMaterial,
    /// The CSR in DER, as ACME's finalize request expects it.
    pub csr_der: Vec<u8>,
}

impl std::fmt::Debug for OrderKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrderKey")
            .field("key", &self.key)
            .field("csr_len", &self.csr_der.len())
            .finish()
    }
}

/// Generates a key of `key_type` and a CSR for `names` (exact DNS names, the
/// first one also the common name). Errors never contain key material.
pub fn generate_order_key(names: &[String], key_type: AcmeKeyType) -> Result<OrderKey, String> {
    if names.is_empty() {
        return Err("at least one name is required".into());
    }
    let algorithm = match key_type {
        AcmeKeyType::EcdsaP256 => &PKCS_ECDSA_P256_SHA256,
        AcmeKeyType::EcdsaP384 => &PKCS_ECDSA_P384_SHA384,
    };
    let key = KeyPair::generate_for(algorithm).map_err(|err| err.to_string())?;
    let mut params = CertificateParams::new(names.to_vec())
        .map_err(|_| "a name cannot be used in a certificate".to_string())?;
    // ACME CAs take the names from the SANs; an empty subject keeps the CSR minimal.
    params.distinguished_name = DistinguishedName::new();
    let csr = params
        .serialize_request(&key)
        .map_err(|err| err.to_string())?;
    Ok(OrderKey {
        csr_der: csr.der().to_vec(),
        key: KeyMaterial::new(KeyKind::Pkcs8, key.serialize_der()),
    })
}

#[cfg(test)]
mod tests {
    use x509_parser::certification_request::X509CertificationRequest;
    use x509_parser::extensions::{GeneralName, ParsedExtension};
    use x509_parser::prelude::FromDer;

    use super::*;

    fn csr_names(der: &[u8]) -> Vec<String> {
        let (_, csr) = X509CertificationRequest::from_der(der).unwrap();
        csr.requested_extensions()
            .into_iter()
            .flatten()
            .filter_map(|ext| match ext {
                ParsedExtension::SubjectAlternativeName(san) => Some(&san.general_names),
                _ => None,
            })
            .flatten()
            .filter_map(|name| match name {
                GeneralName::DNSName(dns) => Some(dns.to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_csr_names_exactly_the_ordered_names_and_is_signed_by_the_key() {
        let names = vec![
            "shop.example.com".to_string(),
            "www.shop.example.com".to_string(),
        ];
        for key_type in [AcmeKeyType::EcdsaP256, AcmeKeyType::EcdsaP384] {
            let order = generate_order_key(&names, key_type).unwrap();
            assert_eq!(csr_names(&order.csr_der), names, "{key_type:?}");
            assert!(order.key.signing_key().is_some(), "{key_type:?}");
        }
    }

    #[test]
    fn every_order_gets_a_fresh_key() {
        let names = vec!["a.example.com".to_string()];
        let first = generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
        let second = generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
        assert_ne!(*first.key.to_sealed_bytes(), *second.key.to_sealed_bytes());
    }

    #[test]
    fn no_names_or_an_unusable_name_is_an_error() {
        assert!(generate_order_key(&[], AcmeKeyType::EcdsaP256).is_err());
        assert!(generate_order_key(&["bücher.example".into()], AcmeKeyType::EcdsaP256).is_err());
    }

    #[test]
    fn debug_shows_neither_the_key_nor_the_csr() {
        let order = generate_order_key(&["a.example.com".into()], AcmeKeyType::EcdsaP256).unwrap();
        let shown = format!("{order:?}");
        assert!(
            shown.contains("csr_len") && shown.contains("Pkcs8"),
            "{shown}"
        );
        assert!(shown.len() < 120, "{shown}");
    }
}
