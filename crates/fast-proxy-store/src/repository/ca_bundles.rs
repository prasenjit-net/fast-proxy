//! CA bundles: public PEM material that upstream groups name for verification.

use std::collections::HashMap;

use fast_proxy_model::{CertificateName, MAX_CA_BUNDLES, MAX_PEM_BYTES};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::expect_changed;
use crate::clock::now_ms;
use crate::StoreError;

/// A bundle to store.
#[derive(Debug, Clone)]
pub struct NewCaBundle {
    pub name: CertificateName,
    pub pem: String,
    /// How many certificates the PEM holds (counted by the caller's parser).
    pub cert_count: u32,
}

/// A stored bundle.
#[derive(Debug, Clone)]
pub struct CaBundleRow {
    pub id: i64,
    pub bundle: NewCaBundle,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

const COLUMNS: &str = "id, name, pem, cert_count, created_at, updated_at";

/// Adds a bundle. A duplicate name is a `Conflict`; the limits are `Invalid`.
pub fn insert(conn: &Connection, bundle: &NewCaBundle) -> Result<i64, StoreError> {
    check_size(bundle)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM ca_bundle", [], |row| row.get(0))?;
    if usize::try_from(count).unwrap_or(usize::MAX) >= MAX_CA_BUNDLES {
        return Err(StoreError::invalid_field(
            "name",
            "at most 16 CA bundles can be stored",
        ));
    }
    let now = now_ms();
    conn.execute(
        "INSERT INTO ca_bundle (name, pem, cert_count, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![bundle.name.as_str(), bundle.pem, bundle.cert_count, now],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Replaces a bundle's PEM (and name); groups name it, so a rename that leaves
/// a group pointing at a missing name is refused.
pub fn update(conn: &Connection, id: i64, bundle: &NewCaBundle) -> Result<(), StoreError> {
    check_size(bundle)?;
    let current = get(conn, id)?;
    if current.bundle.name != bundle.name {
        ensure_unused(conn, current.bundle.name.as_str())?;
    }
    let changed = conn.execute(
        "UPDATE ca_bundle SET name = ?1, pem = ?2, cert_count = ?3, updated_at = ?4 WHERE id = ?5",
        params![
            bundle.name.as_str(),
            bundle.pem,
            bundle.cert_count,
            now_ms(),
            id
        ],
    )?;
    expect_changed(changed, &format!("ca bundle {id}"))
}

/// Deletes a bundle. A `Conflict` while an upstream group names it.
pub fn delete(conn: &Connection, id: i64) -> Result<(), StoreError> {
    let current = get(conn, id)?;
    ensure_unused(conn, current.bundle.name.as_str())?;
    let changed = conn.execute("DELETE FROM ca_bundle WHERE id = ?1", [id])?;
    expect_changed(changed, &format!("ca bundle {id}"))
}

fn ensure_unused(conn: &Connection, name: &str) -> Result<(), StoreError> {
    let using: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upstream_group WHERE tls_ca = ?1",
        [name],
        |row| row.get(0),
    )?;
    if using > 0 {
        return Err(StoreError::Conflict(format!(
            "CA bundle {name} is used by {using} upstream group(s)"
        )));
    }
    Ok(())
}

/// One bundle.
pub fn get(conn: &Connection, id: i64) -> Result<CaBundleRow, StoreError> {
    let row = conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM ca_bundle WHERE id = ?1"),
            [id],
            read,
        )
        .optional()?;
    row.ok_or_else(|| StoreError::NotFound(format!("ca bundle {id}")))?
}

/// Every bundle, ordered by id.
pub fn list(conn: &Connection) -> Result<Vec<CaBundleRow>, StoreError> {
    let mut statement = conn.prepare(&format!("SELECT {COLUMNS} FROM ca_bundle ORDER BY id"))?;
    let rows = statement.query_map([], read)?;
    rows.map(|row| row?).collect()
}

/// Bundle names mapped to their ids.
pub fn ids_by_name(conn: &Connection) -> Result<HashMap<String, i64>, StoreError> {
    let mut statement = conn.prepare("SELECT name, id FROM ca_bundle")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn check_size(bundle: &NewCaBundle) -> Result<(), StoreError> {
    if bundle.pem.len() > MAX_PEM_BYTES {
        return Err(StoreError::invalid_field("pem", "must be at most 256 KiB"));
    }
    Ok(())
}

fn read(row: &Row<'_>) -> rusqlite::Result<Result<CaBundleRow, StoreError>> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let name = match CertificateName::parse(&name) {
        Ok(name) => name,
        Err(err) => {
            return Ok(Err(StoreError::Internal(format!(
                "stored ca bundle {id} is invalid: {err}"
            ))))
        }
    };
    Ok(Ok(CaBundleRow {
        id,
        bundle: NewCaBundle {
            name,
            pem: row.get(2)?,
            cert_count: row.get(3)?,
        },
        created_at_ms: row.get(4)?,
        updated_at_ms: row.get(5)?,
    }))
}
