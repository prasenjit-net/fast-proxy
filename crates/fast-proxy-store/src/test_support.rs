//! Builders shared by the store's unit tests.

use std::time::Duration;

use fast_proxy_model::{
    EndpointAddr, EndpointState, GroupPolicy, HostPattern, RetryPolicy, RoutePrefix, SubsystemId,
    SubsystemName, UpstreamGroupId, UpstreamPrefix,
};
use rusqlite::Connection;

use crate::{
    Database, EndpointFields, Mutation, RouteFields, StoreError, SubsystemFields, UpstreamFields,
    Written,
};

pub fn subsystem_fields(name: &str) -> SubsystemFields {
    SubsystemFields {
        name: SubsystemName::parse(name).unwrap(),
        enabled: true,
        description: String::new(),
    }
}

pub fn upstream_fields(name: &str) -> UpstreamFields {
    UpstreamFields {
        name: name.into(),
        policy: GroupPolicy::default(),
    }
}

pub fn endpoint_fields(addr: &str) -> EndpointFields {
    EndpointFields {
        addr: EndpointAddr::parse(addr).unwrap(),
        weight: 1,
        state: EndpointState::Enabled,
    }
}

/// A group with one endpoint at `addr` and the default policy.
pub fn create_group(
    conn: &Connection,
    subsystem: SubsystemId,
    name: &str,
    addr: &str,
) -> Result<UpstreamGroupId, StoreError> {
    crate::repository::upstreams::create(
        conn,
        subsystem,
        &upstream_fields(name),
        &[endpoint_fields(addr)],
    )
}

pub fn route_fields(prefix: &str, group: UpstreamGroupId) -> RouteFields {
    RouteFields {
        prefix: RoutePrefix::parse(prefix).unwrap(),
        upstream_group_id: group,
        strip_prefix: false,
        upstream_prefix: UpstreamPrefix::default(),
        preserve_host: true,
        connect_timeout: Duration::from_secs(5),
        response_timeout: Duration::from_secs(30),
        retry: RetryPolicy::default(),
        enabled: true,
    }
}

pub fn host(pattern: &str) -> HostPattern {
    HostPattern::parse(pattern).unwrap()
}

/// Runs `op` as one committed change with no extra validation.
pub fn commit<T>(
    db: &mut Database,
    summary: &str,
    op: impl FnOnce(&Connection) -> Result<T, StoreError>,
) -> Result<Written<T>, StoreError> {
    db.write(
        |conn| {
            Ok(Mutation {
                value: op(conn)?,
                summary: summary.to_string(),
            })
        },
        |_| Ok(()),
    )
}

/// A subsystem with one host, one upstream, and a `/` route.
pub struct Fixture {
    pub subsystem: SubsystemId,
    pub group: UpstreamGroupId,
}

pub fn seed(db: &mut Database, name: &str, host_pattern: &str) -> Fixture {
    use crate::repository::{hosts, routes, subsystems};
    commit(db, &format!("seed {name}"), |conn| {
        let subsystem = subsystems::create(conn, &subsystem_fields(name))?;
        hosts::add(conn, subsystem, &host(host_pattern))?;
        let group = create_group(conn, subsystem, "backend", "127.0.0.1:3000")?;
        routes::create(conn, subsystem, &route_fields("/", group))?;
        Ok(Fixture { subsystem, group })
    })
    .unwrap()
    .value
}
