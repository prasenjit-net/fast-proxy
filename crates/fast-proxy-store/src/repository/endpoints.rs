use fast_proxy_model::{
    parse_weight, EndpointId, EndpointState, SubsystemId, UpstreamGroupId, MAX_ENDPOINTS_PER_GROUP,
};
use rusqlite::{params, Connection, OptionalExtension};

use super::{expect_changed, touch};
use crate::inputs::EndpointFields;
use crate::StoreError;

/// Fails with `NotFound` unless the group belongs to the subsystem.
fn ensure_group(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
) -> Result<(), StoreError> {
    let found = conn
        .query_row(
            "SELECT 1 FROM upstream_group WHERE id = ?1 AND subsystem_id = ?2",
            params![group_id.0, subsystem_id.0],
            |_| Ok(()),
        )
        .optional()?;
    found.ok_or_else(|| StoreError::NotFound(format!("upstream {}", group_id.0)))
}

fn count_in_group(conn: &Connection, group_id: UpstreamGroupId) -> Result<usize, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upstream_endpoint WHERE group_id = ?1",
        [group_id.0],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(count).unwrap_or(usize::MAX))
}

/// Inserts one endpoint row, with an explicit id when restoring a revision.
/// `field_prefix` names the weight field in a validation error.
pub(crate) fn insert(
    conn: &Connection,
    id: Option<EndpointId>,
    group_id: UpstreamGroupId,
    fields: &EndpointFields,
    field_prefix: &str,
) -> Result<EndpointId, StoreError> {
    parse_weight(&format!("{field_prefix}.weight"), fields.weight)?;
    conn.execute(
        "INSERT INTO upstream_endpoint (id, group_id, addr, weight, state)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            id.map(|id| id.0),
            group_id.0,
            fields.addr.to_string(),
            fields.weight,
            fields.state.as_str()
        ],
    )?;
    Ok(EndpointId(conn.last_insert_rowid()))
}

/// Adds an endpoint to a group. A duplicate address is a `Conflict`; a group
/// holds at most 64 endpoints.
pub fn add(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    fields: &EndpointFields,
) -> Result<EndpointId, StoreError> {
    ensure_group(conn, subsystem_id, group_id)?;
    if count_in_group(conn, group_id)? >= MAX_ENDPOINTS_PER_GROUP {
        return Err(StoreError::Conflict(
            "a group holds at most 64 endpoints".to_string(),
        ));
    }
    let id = insert(conn, None, group_id, fields, "")?;
    touch(conn, subsystem_id)?;
    Ok(id)
}

/// Replaces an endpoint's address, weight and state.
pub fn update(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    endpoint_id: EndpointId,
    fields: &EndpointFields,
) -> Result<(), StoreError> {
    ensure_group(conn, subsystem_id, group_id)?;
    parse_weight("weight", fields.weight)?;
    let changed = conn.execute(
        "UPDATE upstream_endpoint SET addr = ?1, weight = ?2, state = ?3
         WHERE id = ?4 AND group_id = ?5",
        params![
            fields.addr.to_string(),
            fields.weight,
            fields.state.as_str(),
            endpoint_id.0,
            group_id.0
        ],
    )?;
    expect_changed(changed, &format!("endpoint {}", endpoint_id.0))?;
    touch(conn, subsystem_id)
}

/// Sets only the admin state (enable, drain or disable).
pub fn set_state(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    endpoint_id: EndpointId,
    state: EndpointState,
) -> Result<(), StoreError> {
    ensure_group(conn, subsystem_id, group_id)?;
    let changed = conn.execute(
        "UPDATE upstream_endpoint SET state = ?1 WHERE id = ?2 AND group_id = ?3",
        params![state.as_str(), endpoint_id.0, group_id.0],
    )?;
    expect_changed(changed, &format!("endpoint {}", endpoint_id.0))?;
    touch(conn, subsystem_id)
}

/// Removes an endpoint. The last endpoint of a group cannot be removed
/// (`Conflict`): delete the upstream instead.
pub fn delete(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    endpoint_id: EndpointId,
) -> Result<(), StoreError> {
    ensure_group(conn, subsystem_id, group_id)?;
    if count_in_group(conn, group_id)? <= 1 {
        return Err(StoreError::Conflict(
            "cannot remove the last endpoint of an upstream; delete the upstream instead"
                .to_string(),
        ));
    }
    let changed = conn.execute(
        "DELETE FROM upstream_endpoint WHERE id = ?1 AND group_id = ?2",
        params![endpoint_id.0, group_id.0],
    )?;
    expect_changed(changed, &format!("endpoint {}", endpoint_id.0))?;
    touch(conn, subsystem_id)
}
