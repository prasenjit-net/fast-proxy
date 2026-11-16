//! Storing a certificate an ACME CA issued (K13, ADR 0027): one change, by
//! `system`, that seals the key, replaces the subsystem's ACME certificate in
//! place (so the certificate cache and other instances pick it up as a
//! replacement) or creates it, attaches it, clears the order's challenges and
//! drops the development certificate it supersedes.

use fast_proxy_model::{
    CertificateId, CertificateName, CertificateSource, SubsystemConfig, SubsystemId,
};
use fast_proxy_store::repository::acme::challenges;
use fast_proxy_store::repository::certificates::{self, CertificateRow, NewCertificate};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Connection, Mutation, StoreError};
use fast_proxy_tls::{validate_issued, KeyMaterial, ValidatedCertificate};

use crate::dev_certificates::{attach, dev_name, free_name};
use crate::service::now_ms;
use crate::tls_state::TlsState;
use crate::{Applied, Audited, ChangeContext, ConfigService};

/// What a finished order hands over to be stored.
pub struct IssuedCertificate {
    pub subsystem: SubsystemId,
    /// The chain the CA returned, leaf first.
    pub chain_pem: String,
    /// The key generated for the order.
    pub key: KeyMaterial,
    /// The names the order covered; the chain must cover every one.
    pub names: Vec<String>,
}

impl std::fmt::Debug for IssuedCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedCertificate")
            .field("subsystem", &self.subsystem)
            .field("names", &self.names)
            .finish_non_exhaustive()
    }
}

/// What storing an issued certificate did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredAcmeCertificate {
    pub id: CertificateId,
    /// The subsystem's earlier ACME certificate was replaced in place.
    pub renewed: bool,
    pub not_before_ms: i64,
    pub not_after_ms: i64,
}

impl ConfigService {
    /// Stores and attaches an issued certificate. The chain is checked first
    /// (it must match the key, be current and cover every ordered name); a
    /// subsystem whose ACME switch went off while the order ran is a `Conflict`,
    /// and nothing is stored.
    pub async fn store_acme_certificate(
        &self,
        issued: IssuedCertificate,
    ) -> Result<Applied<StoredAcmeCertificate>, StoreError> {
        let subsystem = issued.subsystem;
        let validated = validate_issued(&issued.chain_pem, issued.key, &issued.names, now_ms())?;
        let renewal = self
            .read(move |conn| Ok(attached_acme_certificate(conn, subsystem)?.is_some()))
            .await?
            .value;
        let action = if renewal {
            "certificate.acme.renew"
        } else {
            "certificate.acme.issue"
        };
        let tls = self.tls.clone();
        let chain_pem = issued.chain_pem;
        self.apply(
            &ChangeContext::system(),
            Audited::new(action, format!("subsystem:{}", subsystem.0)),
            move |conn| store_in_transaction(conn, &tls, subsystem, chain_pem, validated),
        )
        .await
    }
}

fn store_in_transaction(
    conn: &Connection,
    tls: &TlsState,
    subsystem: SubsystemId,
    chain_pem: String,
    validated: ValidatedCertificate,
) -> Result<Mutation<StoredAcmeCertificate>, StoreError> {
    let current = subsystems::get(conn, subsystem)?;
    if !(current.https.enabled && current.https.acme_enabled) {
        return Err(StoreError::Conflict(format!(
            "ACME was turned off for subsystem \"{}\" while the order ran",
            current.name.as_str()
        )));
    }
    let rows = certificates::list(conn)?;
    let replaced = attached_acme_certificate(conn, subsystem)?;
    let (id, name) = match &replaced {
        Some(row) => (row.id, row.certificate.name.clone()),
        None => (
            certificates::next_id(conn)?,
            CertificateName::parse(&free_name(&rows, &acme_name(&current)))?,
        ),
    };
    let sealed = tls
        .vault_for_sealing(conn)?
        .seal_key(id, name.as_str(), &validated.key.to_sealed_bytes())
        .map_err(|err| StoreError::Internal(err.to_string()))?;
    let metadata = validated.metadata;
    let stored = StoredAcmeCertificate {
        id,
        renewed: replaced.is_some(),
        not_before_ms: metadata.not_before_ms,
        not_after_ms: metadata.not_after_ms,
    };
    let certificate = NewCertificate {
        name: name.clone(),
        source: CertificateSource::Acme,
        chain_pem,
        key_enc: sealed.ciphertext,
        key_nonce: sealed.nonce,
        metadata,
    };
    if replaced.is_some() {
        certificates::update(conn, id, &certificate)?;
    } else {
        certificates::insert(conn, id, &certificate)?;
    }
    attach(conn, &current, id)?;
    challenges::clear_subsystem(conn, subsystem)?;
    if let Some(previous) = current
        .https
        .certificate_id
        .filter(|previous| *previous != id)
    {
        drop_superseded_dev_certificate(conn, &current, &rows, previous)?;
    }
    let verb = if stored.renewed { "renewed" } else { "issued" };
    Ok(Mutation {
        value: stored,
        summary: format!(
            "acme certificate {verb}: {} ({})",
            name.as_str(),
            current.name.as_str()
        ),
    })
}

/// `acme-<subsystem>`: the name of a subsystem's ACME certificate.
fn acme_name(subsystem: &SubsystemConfig) -> String {
    format!("acme-{}", subsystem.name.as_str())
}

/// The subsystem's attached certificate, when ACME issued it.
fn attached_acme_certificate(
    conn: &Connection,
    subsystem: SubsystemId,
) -> Result<Option<CertificateRow>, StoreError> {
    let Some(id) = subsystems::get(conn, subsystem)?.https.certificate_id else {
        return Ok(None);
    };
    let row = certificates::get(conn, id)?;
    Ok((row.certificate.source == CertificateSource::Acme).then_some(row))
}

/// Deletes `previous` when it is the subsystem's own development certificate
/// and nothing uses it any more; an uploaded certificate is always kept.
fn drop_superseded_dev_certificate(
    conn: &Connection,
    subsystem: &SubsystemConfig,
    rows: &[CertificateRow],
    previous: CertificateId,
) -> Result<(), StoreError> {
    let dev = dev_name(subsystem);
    let own_dev = rows.iter().any(|row| {
        row.id == previous
            && row.certificate.source == CertificateSource::SelfSigned
            && is_dev_name(row.certificate.name.as_str(), &dev)
    });
    let unused = !certificates::used_by(conn)?.contains_key(&previous);
    if own_dev && unused {
        certificates::delete(conn, previous)?;
    }
    Ok(())
}

/// `dev` itself, or one of the `dev-2`, `dev-3`... names [`free_name`] picks.
fn is_dev_name(name: &str, dev: &str) -> bool {
    name == dev
        || name
            .strip_prefix(dev)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{AcmeKeyType, SubsystemId};

    use super::{is_dev_name, IssuedCertificate};

    #[test]
    fn debug_shows_the_subsystem_and_names_but_not_the_key_or_chain() {
        let names = vec!["shop.example.com".to_string()];
        let order = fast_proxy_tls::generate_order_key(&names, AcmeKeyType::EcdsaP256).unwrap();
        let issued = IssuedCertificate {
            subsystem: SubsystemId(1),
            chain_pem: "-----BEGIN CERTIFICATE-----\nAAAA".into(),
            key: order.key,
            names,
        };
        let shown = format!("{issued:?}");
        assert!(shown.contains("shop.example.com"), "{shown}");
        assert!(
            !shown.contains("BEGIN") && !shown.contains("Pkcs8"),
            "{shown}"
        );
    }

    #[test]
    fn only_the_subsystems_own_development_names_match() {
        assert!(is_dev_name("dev-shop", "dev-shop"));
        assert!(is_dev_name("dev-shop-2", "dev-shop"));
        assert!(!is_dev_name("dev-shop2", "dev-shop"));
        assert!(!is_dev_name("dev-shop-api", "dev-shop"));
        assert!(!is_dev_name("dev-shop-", "dev-shop"));
        assert!(!is_dev_name("my-cert", "dev-shop"));
    }
}
