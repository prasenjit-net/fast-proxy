use std::time::Duration;

use fast_proxy_model::{
    parse_timeout_ms, Config, EndpointAddr, EndpointConfig, HostPattern, RouteConfig, RouteId,
    RoutePrefix, SubsystemConfig, SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId,
    UpstreamPrefix,
};
use rusqlite::Connection;

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
    let mut statement =
        conn.prepare("SELECT id, name, enabled, description FROM subsystem ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, bool>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (id, name, enabled, description) = row?;
        Ok(SubsystemConfig {
            id: SubsystemId(id),
            name: SubsystemName::parse(&name).map_err(|err| corrupt("subsystem name", err))?,
            enabled,
            description,
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
    let mut statement = conn.prepare(
        "SELECT g.id, g.name, e.addr, e.weight
         FROM upstream_group g JOIN upstream_endpoint e ON e.group_id = g.id
         WHERE g.subsystem_id = ?1 ORDER BY g.id, e.id",
    )?;
    let rows = statement.query_map([id.0], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, u32>(3)?,
        ))
    })?;
    let mut groups: Vec<UpstreamGroupConfig> = Vec::new();
    for row in rows {
        let (group_id, name, addr, weight) = row?;
        let endpoint = EndpointConfig {
            addr: EndpointAddr::parse(&addr).map_err(|err| corrupt("endpoint address", err))?,
            weight,
        };
        match groups.last_mut() {
            Some(group) if group.id.0 == group_id => group.endpoints.push(endpoint),
            _ => groups.push(UpstreamGroupConfig {
                id: UpstreamGroupId(group_id),
                name,
                endpoints: vec![endpoint],
            }),
        }
    }
    Ok(groups)
}

fn load_routes(conn: &Connection, id: SubsystemId) -> Result<Vec<RouteConfig>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, prefix, upstream_group_id, strip_prefix, upstream_prefix, preserve_host,
                connect_timeout_ms, response_timeout_ms, enabled
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
            row.get::<_, bool>(8)?,
        ))
    })?;
    rows.map(|row| {
        let (route_id, prefix, group_id, strip, upstream_prefix, preserve, connect, response, on) =
            row?;
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
            enabled: on,
        })
    })
    .collect()
}

fn stored_timeout(field: &str, millis: i64) -> Result<Duration, StoreError> {
    let millis = u64::try_from(millis).map_err(|err| corrupt(field, err))?;
    parse_timeout_ms(field, millis).map_err(|err| corrupt(field, err))
}
