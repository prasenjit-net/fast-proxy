use fast_proxy_model::{HostId, HostPattern, SubsystemId};
use rusqlite::{params, Connection};

use super::{ensure_subsystem, expect_changed, touch};
use crate::StoreError;

/// Adds a host pattern to a subsystem. Patterns are unique across all
/// subsystems; a clash is a `Conflict`.
pub fn add(
    conn: &Connection,
    subsystem_id: SubsystemId,
    pattern: &HostPattern,
) -> Result<HostId, StoreError> {
    ensure_subsystem(conn, subsystem_id)?;
    conn.execute(
        "INSERT INTO subsystem_host (subsystem_id, pattern) VALUES (?1, ?2)",
        params![subsystem_id.0, pattern.to_pattern_string()],
    )?;
    touch(conn, subsystem_id)?;
    Ok(HostId(conn.last_insert_rowid()))
}

/// Removes a host pattern from a subsystem.
pub fn remove(
    conn: &Connection,
    subsystem_id: SubsystemId,
    host_id: HostId,
) -> Result<(), StoreError> {
    let changed = conn.execute(
        "DELETE FROM subsystem_host WHERE id = ?1 AND subsystem_id = ?2",
        params![host_id.0, subsystem_id.0],
    )?;
    expect_changed(changed, &format!("host {}", host_id.0))?;
    touch(conn, subsystem_id)
}

/// A subsystem's host patterns with their row ids, ordered by id.
pub fn list(
    conn: &Connection,
    subsystem_id: SubsystemId,
) -> Result<Vec<(HostId, HostPattern)>, StoreError> {
    ensure_subsystem(conn, subsystem_id)?;
    let mut statement =
        conn.prepare("SELECT id, pattern FROM subsystem_host WHERE subsystem_id = ?1 ORDER BY id")?;
    let rows = statement.query_map([subsystem_id.0], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    rows.map(|row| {
        let (id, pattern) = row?;
        let pattern = HostPattern::parse(&pattern)
            .map_err(|err| StoreError::Internal(format!("stored host \"{pattern}\": {err}")))?;
        Ok((HostId(id), pattern))
    })
    .collect()
}
