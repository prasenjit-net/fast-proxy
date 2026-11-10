//! Certificate rows. Private keys are opaque ciphertext here: this crate never
//! decrypts (the `tls` crate seals and opens), and never logs a row's contents.

use std::collections::HashMap;

use fast_proxy_model::{
    CertificateId, CertificateMetadata, CertificateName, CertificateSource, SubsystemId,
    MAX_CERTIFICATES, MAX_CHAIN_LENGTH, MAX_PEM_BYTES,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::expect_changed;
use crate::clock::now_ms;
use crate::StoreError;

/// Everything stored for one certificate except its id and timestamps.
#[derive(Clone)]
pub struct NewCertificate {
    pub name: CertificateName,
    pub source: CertificateSource,
    /// Public chain, leaf first.
    pub chain_pem: String,
    /// The private key sealed by the vault.
    pub key_enc: Vec<u8>,
    pub key_nonce: Vec<u8>,
    pub metadata: CertificateMetadata,
}

// A manual impl so a stray `{:?}` can never print ciphertext.
impl std::fmt::Debug for NewCertificate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewCertificate")
            .field("name", &self.name)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// A stored certificate.
#[derive(Clone)]
pub struct CertificateRow {
    pub id: CertificateId,
    pub certificate: NewCertificate,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl std::fmt::Debug for CertificateRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertificateRow")
            .field("id", &self.id)
            .field("certificate", &self.certificate)
            .finish_non_exhaustive()
    }
}

const COLUMNS: &str = "id, name, source, chain_pem, key_enc, key_nonce, names, subject, issuer, \
     not_before, not_after, fingerprint, created_at, updated_at";

/// The id the next inserted certificate will get. Call inside the write
/// transaction (it is immediate, so no other writer can take the id): the key
/// is sealed against `(id, name)` before the row exists.
pub fn next_id(conn: &Connection) -> Result<CertificateId, StoreError> {
    let max: Option<i64> =
        conn.query_row("SELECT MAX(id) FROM certificate", [], |row| row.get(0))?;
    Ok(CertificateId(max.unwrap_or(0) + 1))
}

/// Adds a certificate under `id` (from [`next_id`]). A duplicate name is a
/// `Conflict`; the count and size limits are `Invalid`.
pub fn insert(
    conn: &Connection,
    id: CertificateId,
    certificate: &NewCertificate,
) -> Result<(), StoreError> {
    check_limits(conn, certificate)?;
    let now = now_ms();
    conn.execute(
        &format!("INSERT INTO certificate ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)"),
        params![
            id.0,
            certificate.name.as_str(),
            certificate.source.as_str(),
            certificate.chain_pem,
            certificate.key_enc,
            certificate.key_nonce,
            names_json(&certificate.metadata)?,
            certificate.metadata.subject,
            certificate.metadata.issuer,
            certificate.metadata.not_before_ms,
            certificate.metadata.not_after_ms,
            certificate.metadata.fingerprint,
            now,
        ],
    )?;
    Ok(())
}

/// Replaces a certificate's name and material (a rename re-seals the key, so it
/// is a full replace). Bumps `updated_at`, which versions the cache entry.
pub fn update(
    conn: &Connection,
    id: CertificateId,
    certificate: &NewCertificate,
) -> Result<(), StoreError> {
    check_pem_size(&certificate.chain_pem)?;
    let changed = conn.execute(
        "UPDATE certificate SET name = ?1, source = ?2, chain_pem = ?3, key_enc = ?4,
                key_nonce = ?5, names = ?6, subject = ?7, issuer = ?8, not_before = ?9,
                not_after = ?10, fingerprint = ?11, updated_at = ?12
         WHERE id = ?13",
        params![
            certificate.name.as_str(),
            certificate.source.as_str(),
            certificate.chain_pem,
            certificate.key_enc,
            certificate.key_nonce,
            names_json(&certificate.metadata)?,
            certificate.metadata.subject,
            certificate.metadata.issuer,
            certificate.metadata.not_before_ms,
            certificate.metadata.not_after_ms,
            certificate.metadata.fingerprint,
            now_ms(),
            id.0,
        ],
    )?;
    expect_changed(changed, &format!("certificate {}", id.0))
}

/// Deletes a certificate. A `Conflict` naming the subsystems while any uses it.
pub fn delete(conn: &Connection, id: CertificateId) -> Result<(), StoreError> {
    let users = subsystems_using(conn, id)?;
    if !users.is_empty() {
        return Err(StoreError::Conflict(format!(
            "certificate {} is used by subsystem(s): {}",
            id.0,
            users.join(", ")
        )));
    }
    let changed = conn.execute("DELETE FROM certificate WHERE id = ?1", [id.0])?;
    expect_changed(changed, &format!("certificate {}", id.0))
}

fn subsystems_using(conn: &Connection, id: CertificateId) -> Result<Vec<String>, StoreError> {
    let mut statement =
        conn.prepare("SELECT name FROM subsystem WHERE https_certificate_id = ?1 ORDER BY name")?;
    let rows = statement.query_map([id.0], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One certificate.
pub fn get(conn: &Connection, id: CertificateId) -> Result<CertificateRow, StoreError> {
    let row = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM certificate WHERE id = ?1"),
            [id.0],
            read,
        )
        .optional()?;
    row.ok_or_else(|| StoreError::NotFound(format!("certificate {}", id.0)))?
}

/// Every certificate, ordered by id. A row that no longer parses is `Internal`.
pub fn list(conn: &Connection) -> Result<Vec<CertificateRow>, StoreError> {
    let mut statement = conn.prepare(&format!("SELECT {COLUMNS} FROM certificate ORDER BY id"))?;
    let rows = statement.query_map([], read)?;
    rows.map(|row| row?).collect()
}

/// Certificate ids mapped to their names, for revisions.
pub fn names(conn: &Connection) -> Result<HashMap<i64, String>, StoreError> {
    let mut statement = conn.prepare("SELECT id, name FROM certificate")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Which subsystems reference each certificate.
pub fn used_by(conn: &Connection) -> Result<HashMap<CertificateId, Vec<SubsystemId>>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT https_certificate_id, id FROM subsystem
         WHERE https_certificate_id IS NOT NULL ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((CertificateId(row.get(0)?), SubsystemId(row.get(1)?)))
    })?;
    let mut used: HashMap<CertificateId, Vec<SubsystemId>> = HashMap::new();
    for row in rows {
        let (certificate, subsystem) = row?;
        used.entry(certificate).or_default().push(subsystem);
    }
    Ok(used)
}

fn names_json(metadata: &CertificateMetadata) -> Result<String, StoreError> {
    serde_json::to_string(&metadata.names).map_err(|err| StoreError::Internal(err.to_string()))
}

fn check_limits(conn: &Connection, certificate: &NewCertificate) -> Result<(), StoreError> {
    check_pem_size(&certificate.chain_pem)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM certificate", [], |row| row.get(0))?;
    if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_CERTIFICATES {
        return Err(StoreError::invalid_field(
            "name",
            "at most 256 certificates can be stored",
        ));
    }
    Ok(())
}

fn check_pem_size(chain_pem: &str) -> Result<(), StoreError> {
    if chain_pem.len() > MAX_PEM_BYTES {
        return Err(StoreError::invalid_field(
            "certPem",
            "must be at most 256 KiB",
        ));
    }
    if chain_pem.matches("-----BEGIN CERTIFICATE-----").count() > MAX_CHAIN_LENGTH {
        return Err(StoreError::invalid_field(
            "certPem",
            "a chain has at most 10 certificates",
        ));
    }
    Ok(())
}

fn read(row: &Row<'_>) -> rusqlite::Result<Result<CertificateRow, StoreError>> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let source: String = row.get(2)?;
    let names: String = row.get(6)?;
    let certificate = NewCertificate {
        name: match CertificateName::parse(&name) {
            Ok(name) => name,
            Err(err) => return Ok(Err(corrupt(id, err))),
        },
        source: match CertificateSource::parse(&source) {
            Ok(source) => source,
            Err(err) => return Ok(Err(corrupt(id, err))),
        },
        chain_pem: row.get(3)?,
        key_enc: row.get(4)?,
        key_nonce: row.get(5)?,
        metadata: CertificateMetadata {
            names: match serde_json::from_str(&names) {
                Ok(names) => names,
                Err(err) => return Ok(Err(corrupt(id, err))),
            },
            subject: row.get(7)?,
            issuer: row.get(8)?,
            not_before_ms: row.get(9)?,
            not_after_ms: row.get(10)?,
            fingerprint: row.get(11)?,
        },
    };
    Ok(Ok(CertificateRow {
        id: CertificateId(id),
        certificate,
        created_at_ms: row.get(12)?,
        updated_at_ms: row.get(13)?,
    }))
}

fn corrupt(id: i64, err: impl std::fmt::Display) -> StoreError {
    StoreError::Internal(format!("stored certificate {id} is invalid: {err}"))
}
