use std::collections::HashMap;

use fast_proxy_model::{SubsystemConfig, SubsystemId};
use rusqlite::{params, Connection};

use super::{expect_changed, https_columns, load};
use crate::clock::now_ms;
use crate::inputs::SubsystemFields;
use crate::StoreError;

/// Creates a subsystem. A duplicate name is a `Conflict`; a certificate that
/// does not exist is a `Conflict` too (foreign key).
pub fn create(conn: &Connection, fields: &SubsystemFields) -> Result<SubsystemId, StoreError> {
    insert(conn, None, fields)
}

/// Inserts the subsystem row, with an explicit id when restoring a revision.
pub(crate) fn insert(
    conn: &Connection,
    id: Option<SubsystemId>,
    fields: &SubsystemFields,
) -> Result<SubsystemId, StoreError> {
    let now = now_ms();
    let https = https_columns::Bound::new(&fields.https)?;
    conn.execute(
        "INSERT INTO subsystem (id, name, enabled, description, https_enabled,
                                https_certificate_id, https_redirect, hsts_enabled,
                                hsts_max_age_secs, hsts_include_subdomains,
                                created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id.map(|id| id.0),
            fields.name.as_str(),
            fields.enabled,
            fields.description,
            https.enabled,
            https.certificate_id,
            https.redirect,
            https.hsts_enabled,
            https.hsts_max_age_secs,
            https.hsts_include_subdomains,
            now
        ],
    )?;
    Ok(SubsystemId(conn.last_insert_rowid()))
}

/// Replaces a subsystem's editable fields.
pub fn update(
    conn: &Connection,
    id: SubsystemId,
    fields: &SubsystemFields,
) -> Result<(), StoreError> {
    let https = https_columns::Bound::new(&fields.https)?;
    let changed = conn.execute(
        "UPDATE subsystem SET name = ?1, enabled = ?2, description = ?3, https_enabled = ?4,
                https_certificate_id = ?5, https_redirect = ?6, hsts_enabled = ?7,
                hsts_max_age_secs = ?8, hsts_include_subdomains = ?9, updated_at = ?10
         WHERE id = ?11",
        params![
            fields.name.as_str(),
            fields.enabled,
            fields.description,
            https.enabled,
            https.certificate_id,
            https.redirect,
            https.hsts_enabled,
            https.hsts_max_age_secs,
            https.hsts_include_subdomains,
            now_ms(),
            id.0
        ],
    )?;
    expect_changed(changed, &format!("subsystem {}", id.0))
}

/// Deletes a subsystem with its hosts, upstreams, and routes.
pub fn delete(conn: &Connection, id: SubsystemId) -> Result<(), StoreError> {
    let changed = conn.execute("DELETE FROM subsystem WHERE id = ?1", [id.0])?;
    expect_changed(changed, &format!("subsystem {}", id.0))
}

/// One subsystem with everything under it.
pub fn get(conn: &Connection, id: SubsystemId) -> Result<SubsystemConfig, StoreError> {
    load::load_all(conn)?
        .subsystems
        .into_iter()
        .find(|subsystem| subsystem.id == id)
        .ok_or_else(|| StoreError::NotFound(format!("subsystem {}", id.0)))
}

/// When a subsystem was created and last changed, in unix milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubsystemTimes {
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Timestamps of every subsystem, keyed by id.
pub fn times(conn: &Connection) -> Result<HashMap<SubsystemId, SubsystemTimes>, StoreError> {
    let mut statement = conn.prepare("SELECT id, created_at, updated_at FROM subsystem")?;
    let rows = statement.query_map([], |row| {
        Ok((
            SubsystemId(row.get(0)?),
            SubsystemTimes {
                created_at_ms: row.get(1)?,
                updated_at_ms: row.get(2)?,
            },
        ))
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Every subsystem, ordered by id.
pub fn list(conn: &Connection) -> Result<Vec<SubsystemConfig>, StoreError> {
    Ok(load::load_all(conn)?.subsystems)
}
