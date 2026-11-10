use fast_proxy_model::{SubsystemId, UpstreamGroupId, MAX_ENDPOINTS_PER_GROUP};
use rusqlite::Connection;

use super::{ca_bundles, endpoints, ensure_subsystem, expect_changed, policy_columns, touch};
use crate::inputs::{EndpointFields, UpstreamFields};
use crate::StoreError;

/// Creates an upstream group together with its endpoints, atomically. At
/// least one and at most 64 endpoints are required; a duplicate group name
/// within the subsystem, or a duplicate address, is a `Conflict`.
pub fn create(
    conn: &Connection,
    subsystem_id: SubsystemId,
    fields: &UpstreamFields,
    endpoint_fields: &[EndpointFields],
) -> Result<UpstreamGroupId, StoreError> {
    ensure_subsystem(conn, subsystem_id)?;
    check_endpoint_count(endpoint_fields.len())?;
    ensure_ca_bundle_exists(conn, &fields.policy.tls.ca)?;
    let group_id = insert(conn, None, subsystem_id, fields)?;
    for (index, endpoint) in endpoint_fields.iter().enumerate() {
        endpoints::insert(
            conn,
            None,
            group_id,
            endpoint,
            &format!("endpoints.{index}"),
        )?;
    }
    touch(conn, subsystem_id)?;
    Ok(group_id)
}

/// A group may only name a CA bundle that exists (`tls.ca`); an empty name means
/// the OS trust store. A rollback is checked separately, naming the bundle.
fn ensure_ca_bundle_exists(conn: &Connection, name: &str) -> Result<(), StoreError> {
    if name.is_empty() || ca_bundles::ids_by_name(conn)?.contains_key(name) {
        return Ok(());
    }
    Err(StoreError::invalid_field(
        "tls.ca",
        "no CA bundle has this name; upload it first",
    ))
}

fn check_endpoint_count(count: usize) -> Result<(), StoreError> {
    if count == 0 || count > MAX_ENDPOINTS_PER_GROUP {
        return Err(StoreError::invalid_field(
            "endpoints",
            "a group needs between 1 and 64 endpoints",
        ));
    }
    Ok(())
}

/// Inserts the group row, with an explicit id when restoring a revision.
pub(crate) fn insert(
    conn: &Connection,
    id: Option<UpstreamGroupId>,
    subsystem_id: SubsystemId,
    fields: &UpstreamFields,
) -> Result<UpstreamGroupId, StoreError> {
    fields.policy.validate()?;
    let bound = policy_columns::Bound::new(&fields.policy);
    let mut params = bound.named();
    let id = id.map(|id| id.0);
    params.push((":id", &id));
    params.push((":subsystem_id", &subsystem_id.0));
    params.push((":name", &fields.name));
    conn.execute(
        &format!(
            "INSERT INTO upstream_group (id, subsystem_id, name, {})
             VALUES (:id, :subsystem_id, :name, {})",
            policy_columns::INSERT_COLUMNS,
            policy_columns::INSERT_VALUES
        ),
        params.as_slice(),
    )?;
    Ok(UpstreamGroupId(conn.last_insert_rowid()))
}

/// Replaces an upstream group's name and policy; endpoints are untouched.
pub fn update(
    conn: &Connection,
    subsystem_id: SubsystemId,
    group_id: UpstreamGroupId,
    fields: &UpstreamFields,
) -> Result<(), StoreError> {
    fields.policy.validate()?;
    ensure_ca_bundle_exists(conn, &fields.policy.tls.ca)?;
    let bound = policy_columns::Bound::new(&fields.policy);
    let mut params = bound.named();
    params.push((":id", &group_id.0));
    params.push((":subsystem_id", &subsystem_id.0));
    params.push((":name", &fields.name));
    let changed = conn.execute(
        &format!(
            "UPDATE upstream_group SET name = :name, {}
             WHERE id = :id AND subsystem_id = :subsystem_id",
            policy_columns::UPDATE_SET
        ),
        params.as_slice(),
    )?;
    expect_changed(changed, &format!("upstream {}", group_id.0))?;
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
        rusqlite::params![group_id.0, subsystem_id.0],
    )?;
    expect_changed(changed, &format!("upstream {}", group_id.0))?;
    touch(conn, subsystem_id)
}
