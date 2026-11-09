use std::collections::HashMap;

use fast_proxy_model::{SubsystemConfig, SubsystemId};
use rusqlite::{params, Connection};

use super::{expect_changed, load};
use crate::clock::now_ms;
use crate::inputs::SubsystemFields;
use crate::StoreError;

/// Creates a subsystem. A duplicate name is a `Conflict`.
pub fn create(conn: &Connection, fields: &SubsystemFields) -> Result<SubsystemId, StoreError> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO subsystem (name, enabled, description, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![
            fields.name.as_str(),
            fields.enabled,
            fields.description,
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
    let changed = conn.execute(
        "UPDATE subsystem SET name = ?1, enabled = ?2, description = ?3, updated_at = ?4
         WHERE id = ?5",
        params![
            fields.name.as_str(),
            fields.enabled,
            fields.description,
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
