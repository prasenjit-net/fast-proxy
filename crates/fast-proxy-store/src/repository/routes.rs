use fast_proxy_model::{RouteId, SubsystemId, UpstreamGroupId};
use rusqlite::{params, Connection, OptionalExtension};

use super::{ensure_subsystem, expect_changed, touch};
use crate::inputs::RouteFields;
use crate::StoreError;

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// A route may only point at an upstream of its own subsystem.
fn ensure_group_in_subsystem(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
) -> Result<(), StoreError> {
    let owner: Option<i64> = conn
        .query_row(
            "SELECT subsystem_id FROM upstream_group WHERE id = ?1",
            [group_id.0],
            |row| row.get(0),
        )
        .optional()?;
    if owner != Some(subsystem_id.0) {
        return Err(StoreError::invalid_field(
            "upstream_group_id",
            "must be an upstream of this subsystem",
        ));
    }
    Ok(())
}

/// Creates a route. A duplicate prefix within the subsystem is a `Conflict`.
pub fn create(
    conn: &Connection,
    subsystem_id: SubsystemId,
    fields: &RouteFields,
) -> Result<RouteId, StoreError> {
    ensure_subsystem(conn, subsystem_id)?;
    ensure_group_in_subsystem(conn, subsystem_id, fields.upstream_group_id)?;
    conn.execute(
        "INSERT INTO route (subsystem_id, prefix, upstream_group_id, strip_prefix,
                            upstream_prefix, preserve_host, connect_timeout_ms,
                            response_timeout_ms, enabled)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            subsystem_id.0,
            fields.prefix.as_str(),
            fields.upstream_group_id.0,
            fields.strip_prefix,
            fields.upstream_prefix.as_str(),
            fields.preserve_host,
            millis(fields.connect_timeout),
            millis(fields.response_timeout),
            fields.enabled,
        ],
    )?;
    touch(conn, subsystem_id)?;
    Ok(RouteId(conn.last_insert_rowid()))
}

/// Replaces a route's editable fields.
pub fn update(
    conn: &Connection,
    subsystem_id: SubsystemId,
    route_id: RouteId,
    fields: &RouteFields,
) -> Result<(), StoreError> {
    ensure_group_in_subsystem(conn, subsystem_id, fields.upstream_group_id)?;
    let changed = conn.execute(
        "UPDATE route SET prefix = ?1, upstream_group_id = ?2, strip_prefix = ?3,
                          upstream_prefix = ?4, preserve_host = ?5, connect_timeout_ms = ?6,
                          response_timeout_ms = ?7, enabled = ?8
         WHERE id = ?9 AND subsystem_id = ?10",
        params![
            fields.prefix.as_str(),
            fields.upstream_group_id.0,
            fields.strip_prefix,
            fields.upstream_prefix.as_str(),
            fields.preserve_host,
            millis(fields.connect_timeout),
            millis(fields.response_timeout),
            fields.enabled,
            route_id.0,
            subsystem_id.0,
        ],
    )?;
    expect_changed(changed, &format!("route {}", route_id.0))?;
    touch(conn, subsystem_id)
}

/// Deletes a route.
pub fn delete(
    conn: &Connection,
    subsystem_id: SubsystemId,
    route_id: RouteId,
) -> Result<(), StoreError> {
    let changed = conn.execute(
        "DELETE FROM route WHERE id = ?1 AND subsystem_id = ?2",
        params![route_id.0, subsystem_id.0],
    )?;
    expect_changed(changed, &format!("route {}", route_id.0))?;
    touch(conn, subsystem_id)
}
