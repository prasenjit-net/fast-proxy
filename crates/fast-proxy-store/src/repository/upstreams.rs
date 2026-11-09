use fast_proxy_model::{SubsystemId, UpstreamGroupId};
use rusqlite::{params, Connection};

use super::{ensure_subsystem, expect_changed, touch};
use crate::inputs::UpstreamFields;
use crate::StoreError;

/// Creates an upstream group with its single endpoint. A duplicate name
/// within the subsystem is a `Conflict`.
pub fn create(
    conn: &Connection,
    subsystem_id: SubsystemId,
    fields: &UpstreamFields,
) -> Result<UpstreamGroupId, StoreError> {
    ensure_subsystem(conn, subsystem_id)?;
    conn.execute(
        "INSERT INTO upstream_group (subsystem_id, name) VALUES (?1, ?2)",
        params![subsystem_id.0, fields.name],
    )?;
    let group_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO upstream_endpoint (group_id, addr, weight) VALUES (?1, ?2, ?3)",
        params![group_id, fields.addr.to_string(), fields.weight],
    )?;
    touch(conn, subsystem_id)?;
    Ok(UpstreamGroupId(group_id))
}

/// Replaces an upstream group's name and endpoint.
pub fn update(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    fields: &UpstreamFields,
) -> Result<(), StoreError> {
    let changed = conn.execute(
        "UPDATE upstream_group SET name = ?1 WHERE id = ?2 AND subsystem_id = ?3",
        params![fields.name, group_id.0, subsystem_id.0],
    )?;
    expect_changed(changed, &format!("upstream {}", group_id.0))?;
    conn.execute(
        "UPDATE upstream_endpoint SET addr = ?1, weight = ?2 WHERE group_id = ?3",
        params![fields.addr.to_string(), fields.weight, group_id.0],
    )?;
    touch(conn, subsystem_id)
}

/// Deletes an upstream group. Fails with `Conflict` while a route uses it.
pub fn delete(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
) -> Result<(), StoreError> {
    let routes_using: i64 = conn.query_row(
        "SELECT COUNT(*) FROM route WHERE upstream_group_id = ?1",
        [group_id.0],
        |row| row.get(0),
    )?;
    if routes_using > 0 {
        return Err(StoreError::Conflict(format!(
            "upstream {} is used by {routes_using} route(s)",
            group_id.0
        )));
    }
    let changed = conn.execute(
        "DELETE FROM upstream_group WHERE id = ?1 AND subsystem_id = ?2",
        params![group_id.0, subsystem_id.0],
    )?;
    expect_changed(changed, &format!("upstream {}", group_id.0))?;
    touch(conn, subsystem_id)
}
