//! Certificate writes. Every operation goes through [`ConfigService::apply`]
//! like any other change: the key is sealed and the rows written inside the
//! write transaction, development certificates are reconciled, the would-be
//! snapshot is compiled with the would-be certificates as the veto, and only
//! then is the change committed and swapped in (ADR 0011).
//!
//! Certificate material never enters a revision: a material change is recorded
//! as a summary-only revision (the configuration is unchanged).

use fast_proxy_model::{
    CertificateId, CertificateMetadata, CertificateName, CertificateSource, FieldError,
    HostPattern, HttpsPolicy, SubsystemId,
};
use fast_proxy_store::repository::certificates::{self, NewCertificate};
use fast_proxy_store::repository::subsystems;
use fast_proxy_store::{Connection, Mutation, StoreError, SubsystemFields};
use fast_proxy_tls::{
    generate_self_signed, parse_chain, read_metadata, validate_with_key, CertEntry,
    CertificateState, KeyMaterial,
};

use crate::service::now_ms;
use crate::{Applied, ConfigService};

/// What an upload or replacement produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadOutcome {
    pub id: CertificateId,
    /// Non-fatal findings, such as a certificate expiring soon.
    pub warnings: Vec<String>,
}

/// A certificate as the admin plane lists it: metadata only, never key bytes.
#[derive(Debug, Clone)]
pub struct CertificateSummary {
    pub id: CertificateId,
    pub name: CertificateName,
    pub source: CertificateSource,
    pub metadata: CertificateMetadata,
    pub state: CertificateState,
    pub used_by: Vec<SubsystemId>,
    pub updated_at_ms: i64,
}

/// An edit of a subsystem's own fields.
#[derive(Debug, Clone)]
pub struct SubsystemEdit {
    pub name: fast_proxy_model::SubsystemName,
    pub enabled: bool,
    pub description: String,
    /// The new HTTPS settings; `None` keeps the stored ones.
    pub https: Option<HttpsPolicy>,
}

/// The parts of a certificate an admin may change; `None` keeps the stored one.
#[derive(Debug, Clone, Default)]
pub struct CertificateChange {
    pub name: Option<String>,
    pub cert_pem: Option<String>,
    pub key_pem: Option<String>,
}

impl ConfigService {
    /// Stores an uploaded certificate and key, which seals the key.
    pub async fn upload_certificate(
        &self,
        name: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<Applied<UploadOutcome>, StoreError> {
        let name = CertificateName::parse(name)?;
        let validated = fast_proxy_tls::validate_upload(cert_pem, key_pem, now_ms())?;
        let cert_pem = cert_pem.to_string();
        let tls = self.tls.clone();
        self.apply(move |conn| {
            let outcome = insert_uploaded(conn, &tls, &name, cert_pem, validated)?;
            Ok(Mutation {
                value: outcome,
                summary: format!("certificate uploaded: {}", name.as_str()),
            })
        })
        .await
    }

    /// Uploads a certificate and attaches it to `subsystem` in one write. It must
    /// cover at least one of the subsystem's hosts (`certPem` is named
    /// otherwise); nothing is stored when it does not. The result lists the
    /// hosts it does not cover.
    pub async fn upload_certificate_for_subsystem(
        &self,
        subsystem: SubsystemId,
        name: &str,
        cert_pem: &str,
        key_pem: &str,
    ) -> Result<Applied<(UploadOutcome, Vec<String>)>, StoreError> {
        let name = CertificateName::parse(name)?;
        let validated = fast_proxy_tls::validate_upload(cert_pem, key_pem, now_ms())?;
        let cert_pem = cert_pem.to_string();
        let tls = self.tls.clone();
        self.apply(move |conn| {
            let current = subsystems::get(conn, subsystem)?;
            let uncovered = uncovered_hosts(&current.hosts, &validated.metadata);
            if !current.hosts.is_empty() && uncovered.len() == current.hosts.len() {
                return Err(FieldError::new(
                    "certPem",
                    "the certificate covers none of this subsystem's hosts",
                )
                .into());
            }
            let outcome = insert_uploaded(conn, &tls, &name, cert_pem, validated)?;
            subsystems::update(
                conn,
                subsystem,
                &SubsystemFields {
                    name: current.name.clone(),
                    enabled: current.enabled,
                    description: current.description.clone(),
                    https: HttpsPolicy {
                        certificate_id: Some(outcome.id),
                        ..current.https
                    },
                },
            )?;
            Ok(Mutation {
                value: (outcome, uncovered),
                summary: format!(
                    "certificate uploaded: {} (attached to {})",
                    name.as_str(),
                    current.name.as_str()
                ),
            })
        })
        .await
    }

    /// Generates and stores a self-signed certificate for `names`.
    pub async fn generate_self_signed(
        &self,
        name: &str,
        names: Vec<String>,
    ) -> Result<Applied<CertificateId>, StoreError> {
        let name = CertificateName::parse(name)?;
        let tls = self.tls.clone();
        self.apply(move |conn| {
            let generated = generate_self_signed(&names, now_ms())
                .map_err(|err| StoreError::invalid_field("names", &err))?;
            let metadata = read_metadata(&parse_chain(&generated.chain_pem)?[0])?;
            let vault = tls.vault_for_sealing(conn)?;
            let id = certificates::next_id(conn)?;
            let sealed = vault
                .seal_key(id, name.as_str(), &generated.key.to_sealed_bytes())
                .map_err(|err| StoreError::Internal(err.to_string()))?;
            certificates::insert(
                conn,
                id,
                &NewCertificate {
                    name: name.clone(),
                    source: CertificateSource::SelfSigned,
                    chain_pem: generated.chain_pem,
                    key_enc: sealed.ciphertext,
                    key_nonce: sealed.nonce,
                    metadata,
                },
            )?;
            Ok(Mutation {
                value: id,
                summary: format!("certificate generated: {}", name.as_str()),
            })
        })
        .await
    }

    /// Renames a certificate and/or replaces its chain and key. A new chain
    /// without a key must match the stored key (a renewal with the same key);
    /// the certificate must still cover at least one host of every subsystem
    /// that uses it. Replacing material makes the certificate `uploaded`, so
    /// it is never auto-replaced.
    pub async fn replace_certificate(
        &self,
        id: CertificateId,
        change: CertificateChange,
    ) -> Result<Applied<UploadOutcome>, StoreError> {
        let new_name = change
            .name
            .as_deref()
            .map(CertificateName::parse)
            .transpose()?;
        let tls = self.tls.clone();
        self.apply(move |conn| {
            let row = certificates::get(conn, id)?;
            let old = &row.certificate;
            let name = new_name.unwrap_or_else(|| old.name.clone());
            let vault = tls.vault_for_sealing(conn)?;
            let key = match &change.key_pem {
                Some(pem) => fast_proxy_tls::parse_key(pem)?,
                None => {
                    let plain = vault
                        .open_key(id, old.name.as_str(), &sealed_of(old))
                        .map_err(|_| {
                            StoreError::Conflict("the stored private key cannot be read".into())
                        })?;
                    KeyMaterial::from_sealed_bytes(&plain).ok_or_else(|| {
                        StoreError::Internal("the stored private key is corrupt".into())
                    })?
                }
            };
            let material_changed = change.cert_pem.is_some() || change.key_pem.is_some();
            let (chain_pem, metadata, warnings) = if material_changed {
                let chain_pem = change
                    .cert_pem
                    .clone()
                    .unwrap_or_else(|| old.chain_pem.clone());
                let validated = validate_with_key(&chain_pem, key_clone(&key)?, now_ms())?;
                ensure_users_covered(conn, id, &validated.metadata)?;
                (chain_pem, validated.metadata, validated.warnings)
            } else {
                (old.chain_pem.clone(), old.metadata.clone(), Vec::new())
            };
            let sealed = vault
                .seal_key(id, name.as_str(), &key.to_sealed_bytes())
                .map_err(|err| StoreError::Internal(err.to_string()))?;
            certificates::update(
                conn,
                id,
                &NewCertificate {
                    name: name.clone(),
                    source: if material_changed {
                        CertificateSource::Uploaded
                    } else {
                        old.source
                    },
                    chain_pem,
                    key_enc: sealed.ciphertext,
                    key_nonce: sealed.nonce,
                    metadata,
                },
            )?;
            Ok(Mutation {
                value: UploadOutcome { id, warnings },
                summary: format!("certificate replaced: {}", name.as_str()),
            })
        })
        .await
    }

    /// Deletes a certificate; a `Conflict` while a subsystem uses it.
    pub async fn delete_certificate(&self, id: CertificateId) -> Result<Applied<()>, StoreError> {
        self.apply(move |conn| {
            let name = certificates::get(conn, id)?.certificate.name;
            certificates::delete(conn, id)?;
            Ok(Mutation {
                value: (),
                summary: format!("certificate deleted: {}", name.as_str()),
            })
        })
        .await
    }

    /// Applies an edit of a subsystem's fields, and of its HTTPS settings when
    /// the edit carries them (`None` keeps the stored ones). Attaching a
    /// certificate needs it to cover at least one of the subsystem's hosts; the
    /// result lists the hosts it does not cover. Enabling HTTPS with no
    /// certificate (or detaching one) while TLS is on generates a development
    /// certificate in the same write.
    pub async fn update_subsystem(
        &self,
        id: SubsystemId,
        edit: SubsystemEdit,
    ) -> Result<Applied<Vec<String>>, StoreError> {
        if let Some(https) = &edit.https {
            https.validate()?;
        }
        self.apply(move |conn| {
            let current = subsystems::get(conn, id)?;
            let summary = format!("subsystem updated: {}", edit.name.as_str());
            let uncovered = write_subsystem(conn, id, &current, edit)?;
            Ok(Mutation {
                value: uncovered,
                summary,
            })
        })
        .await
    }

    /// Replaces only a subsystem's HTTPS settings (see [`Self::update_subsystem`]).
    pub async fn set_subsystem_https(
        &self,
        id: SubsystemId,
        https: HttpsPolicy,
    ) -> Result<Applied<Vec<String>>, StoreError> {
        https.validate()?;
        self.apply(move |conn| {
            let current = subsystems::get(conn, id)?;
            let summary = format!("https settings changed: {}", current.name.as_str());
            let edit = SubsystemEdit {
                name: current.name.clone(),
                enabled: current.enabled,
                description: current.description.clone(),
                https: Some(https),
            };
            let uncovered = write_subsystem(conn, id, &current, edit)?;
            Ok(Mutation {
                value: uncovered,
                summary,
            })
        })
        .await
    }

    /// Every certificate with its state and the subsystems using it (metadata
    /// only; key material never leaves the vault).
    pub async fn list_certificates(&self) -> Result<Vec<CertificateSummary>, StoreError> {
        let cache = self.tls.cache();
        let applied = self
            .read(|conn| Ok((certificates::list(conn)?, certificates::used_by(conn)?)))
            .await?;
        let (rows, mut used_by) = applied.value;
        let now = now_ms();
        Ok(rows
            .into_iter()
            .map(|row| {
                let unreadable = matches!(cache.entry(row.id), Some(CertEntry::Unreadable(_)));
                let metadata = row.certificate.metadata;
                CertificateSummary {
                    id: row.id,
                    state: CertificateState::of(metadata.not_after_ms, unreadable, now),
                    name: row.certificate.name,
                    source: row.certificate.source,
                    metadata,
                    used_by: used_by.remove(&row.id).unwrap_or_default(),
                    updated_at_ms: row.updated_at_ms,
                }
            })
            .collect())
    }
}

/// Writes a subsystem edit inside the transaction, enforcing the coverage rule
/// for an attached certificate. Returns the hosts the certificate does not cover.
fn write_subsystem(
    conn: &Connection,
    id: SubsystemId,
    current: &fast_proxy_model::SubsystemConfig,
    edit: SubsystemEdit,
) -> Result<Vec<String>, StoreError> {
    let https = edit.https.unwrap_or(current.https);
    let mut uncovered = Vec::new();
    if let Some(certificate_id) = https.certificate_id {
        let metadata = certificates::get(conn, certificate_id)
            .map_err(|_| StoreError::invalid_field("https.certificateId", "does not exist"))?
            .certificate
            .metadata;
        uncovered = uncovered_hosts(&current.hosts, &metadata);
        if !current.hosts.is_empty() && uncovered.len() == current.hosts.len() {
            return Err(StoreError::invalid_field(
                "https.certificateId",
                "the certificate covers none of this subsystem's hosts",
            ));
        }
    }
    subsystems::update(
        conn,
        id,
        &SubsystemFields {
            name: edit.name,
            enabled: edit.enabled,
            description: edit.description,
            https,
        },
    )?;
    Ok(uncovered)
}

/// Seals the key and stores an uploaded certificate inside the write transaction.
fn insert_uploaded(
    conn: &Connection,
    tls: &crate::tls_state::TlsState,
    name: &CertificateName,
    cert_pem: String,
    validated: fast_proxy_tls::ValidatedCertificate,
) -> Result<UploadOutcome, StoreError> {
    let vault = tls.vault_for_sealing(conn)?;
    let id = certificates::next_id(conn)?;
    let sealed = vault
        .seal_key(id, name.as_str(), &validated.key.to_sealed_bytes())
        .map_err(|err| StoreError::Internal(err.to_string()))?;
    certificates::insert(
        conn,
        id,
        &NewCertificate {
            name: name.clone(),
            source: CertificateSource::Uploaded,
            chain_pem: cert_pem,
            key_enc: sealed.ciphertext,
            key_nonce: sealed.nonce,
            metadata: validated.metadata,
        },
    )?;
    Ok(UploadOutcome {
        id,
        warnings: validated.warnings,
    })
}

fn sealed_of(certificate: &NewCertificate) -> fast_proxy_tls::Sealed {
    fast_proxy_tls::Sealed {
        nonce: certificate.key_nonce.clone(),
        ciphertext: certificate.key_enc.clone(),
    }
}

/// The key bytes again, since validation consumes one copy and sealing needs another.
fn key_clone(key: &KeyMaterial) -> Result<KeyMaterial, StoreError> {
    KeyMaterial::from_sealed_bytes(&key.to_sealed_bytes())
        .ok_or_else(|| StoreError::Internal("a private key could not be copied".into()))
}

/// The host patterns no name of the certificate covers.
fn uncovered_hosts(hosts: &[HostPattern], metadata: &CertificateMetadata) -> Vec<String> {
    hosts
        .iter()
        .filter(|host| !metadata.covers(host))
        .map(HostPattern::to_pattern_string)
        .collect()
}

/// A replacement certificate must still cover one host of each subsystem using it.
fn ensure_users_covered(
    conn: &Connection,
    id: CertificateId,
    metadata: &CertificateMetadata,
) -> Result<(), StoreError> {
    let config = fast_proxy_store::repository::load::load_all(conn)?;
    for subsystem in config
        .subsystems
        .iter()
        .filter(|subsystem| subsystem.https.certificate_id == Some(id))
    {
        let uncovered = uncovered_hosts(&subsystem.hosts, metadata);
        if !subsystem.hosts.is_empty() && uncovered.len() == subsystem.hosts.len() {
            return Err(FieldError::new(
                "certPem",
                format!(
                    "no longer covers any host of subsystem \"{}\"",
                    subsystem.name.as_str()
                ),
            )
            .into());
        }
    }
    Ok(())
}
