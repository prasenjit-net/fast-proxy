use std::time::Duration;

use fast_proxy_model::{
    parse_timeout_ms, parse_weight, Config, EndpointAddr, EndpointConfig, EndpointId,
    EndpointState, HostPattern, RetryPolicy, RouteConfig, RouteId, RoutePrefix, SubsystemConfig,
    SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId, UpstreamPrefix,
};
use rusqlite::Connection;

use super::{https_columns, policy_columns};
use crate::StoreError;

/// Reads the whole configuration, ordered by id so the result is deterministic.
/// Stored values that no longer parse are reported as `Internal` (corruption).
pub fn load_all(conn: &Connection) -> Result<Config, StoreError> {
    let mut subsystems = load_subsystems(conn)?;
    for subsystem in &mut subsystems {
        subsystem.hosts = load_hosts(conn, subsystem.id)?;
        subsystem.upstream_groups = load_groups(conn, subsystem.id)?;
        subsystem.routes = load_routes(conn, subsystem.id)?;
    }
    Ok(Config { subsystems })
}

fn corrupt(what: &str, err: impl std::fmt::Display) -> StoreError {
    StoreError::Internal(format!("stored {what} is invalid: {err}"))
}

fn load_subsystems(conn: &Connection) -> Result<Vec<SubsystemConfig>, StoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT id, name, enabled, description, {} FROM subsystem ORDER BY id",
        https_columns::SELECT_LIST
    ))?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, String>(3)?,
            https_columns::read(row, 4)?,
        ))
    })?;
    rows.map(|row| {
        let (id, name, enabled, description, https) = row?;
        Ok(SubsystemConfig {
            id: SubsystemId(id),
            name: SubsystemName::parse(&name).map_err(|err| corrupt("subsystem name", err))?,
            enabled,
            description,
            https: https?,
            hosts: Vec::new(),
            upstream_groups: Vec::new(),
            routes: Vec::new(),
        })
    })
    .collect()
}

fn load_hosts(conn: &Connection, id: SubsystemId) -> Result<Vec<HostPattern>, StoreError> {
    let mut statement =
        conn.prepare("SELECT pattern FROM subsystem_host WHERE subsystem_id = ?1 ORDER BY id")?;
    let rows = statement.query_map([id.0], |row| row.get::<_, String>(0))?;
    rows.map(|pattern| HostPattern::parse(&pattern?).map_err(|err| corrupt("host pattern", err)))
        .collect()
}

fn load_groups(conn: &Connection, id: SubsystemId) -> Result<Vec<UpstreamGroupConfig>, StoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT id, name, {} FROM upstream_group WHERE subsystem_id = ?1 ORDER BY id",
        policy_columns::SELECT_LIST
    ))?;
    let rows = statement.query_map([id.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            policy_columns::read(row, 2)?,
        ))
    })?;
    rows.map(|row| {
        let (group_id, name, policy) = row?;
        Ok(UpstreamGroupConfig {
            id: UpstreamGroupId(group_id),
            name,
            policy: policy?,
            endpoints: load_endpoints(conn, UpstreamGroupId(group_id))?,
        })
    })
    .collect()
}

fn load_endpoints(
    conn: &Connection,
    group_id: UpstreamGroupId,
) -> Result<Vec<EndpointConfig>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, addr, weight, state FROM upstream_endpoint WHERE group_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([group_id.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, u32>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (id, addr, weight, state) = row?;
        Ok(EndpointConfig {
            id: EndpointId(id),
            addr: EndpointAddr::parse(&addr).map_err(|err| corrupt("endpoint address", err))?,
            weight: parse_weight("weight", weight)
                .map_err(|err| corrupt("endpoint weight", err))?,
            state: EndpointState::parse(&state).map_err(|err| corrupt("endpoint state", err))?,
        })
    })
    .collect()
}

fn load_routes(conn: &Connection, id: SubsystemId) -> Result<Vec<RouteConfig>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, prefix, upstream_group_id, strip_prefix, upstream_prefix, preserve_host,
                connect_timeout_ms, response_timeout_ms, retry_max_attempts,
                retry_idempotent_on_5xx, retry_on_timeout, enabled
         FROM route WHERE subsystem_id = ?1 ORDER BY id",
    )?;
    let rows = statement.query_map([id.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, bool>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, bool>(5)?,
            row.get::<_, i64>(6)?,
            row.get::<_, i64>(7)?,
            (
                row.get::<_, u32>(8)?,
                row.get::<_, bool>(9)?,
                row.get::<_, bool>(10)?,
            ),
            row.get::<_, bool>(11)?,
        ))
    })?;
    rows.map(|row| {
        let (
            route_id,
            prefix,
            group_id,
            strip,
            upstream_prefix,
            preserve,
            connect,
            response,
            retry,
            on,
        ) = row?;
        Ok(RouteConfig {
            id: RouteId(route_id),
            prefix: RoutePrefix::parse(&prefix).map_err(|err| corrupt("route prefix", err))?,
            upstream_group_id: UpstreamGroupId(group_id),
            strip_prefix: strip,
            upstream_prefix: UpstreamPrefix::parse(&upstream_prefix)
                .map_err(|err| corrupt("upstream prefix", err))?,
            preserve_host: preserve,
            connect_timeout: stored_timeout("connect_timeout_ms", connect)?,
            response_timeout: stored_timeout("response_timeout_ms", response)?,
            retry: stored_retry(retry)?,
            enabled: on,
        })
    })
    .collect()
}

fn stored_timeout(field: &str, millis: i64) -> Result<Duration, StoreError> {
    let millis = u64::try_from(millis).map_err(|err| corrupt(field, err))?;
    parse_timeout_ms(field, millis).map_err(|err| corrupt(field, err))
}

fn stored_retry(
    (max_attempts, idempotent_on_5xx, on_timeout): (u32, bool, bool),
) -> Result<RetryPolicy, StoreError> {
    let retry = RetryPolicy {
        max_attempts,
        idempotent_on_5xx,
        on_timeout,
    };
    retry
        .validate()
        .map_err(|err| corrupt("route retry policy", err))?;
    Ok(retry)
}
