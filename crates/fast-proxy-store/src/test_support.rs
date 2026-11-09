//! Builders shared by the store's unit tests.

use std::time::Duration;

use fast_proxy_model::{
    EndpointAddr, HostPattern, RoutePrefix, SubsystemId, SubsystemName, UpstreamGroupId,
    UpstreamPrefix,
};
use rusqlite::Connection;

use crate::{
    Database, Mutation, RouteFields, StoreError, SubsystemFields, UpstreamFields, Written,
};

pub fn subsystem_fields(name: &str) -> SubsystemFields {
    SubsystemFields {
        name: SubsystemName::parse(name).unwrap(),
        enabled: true,
        description: String::new(),
    }
}

pub fn upstream_fields(name: &str, addr: &str) -> UpstreamFields {
    UpstreamFields {
        name: name.into(),
        addr: EndpointAddr::parse(addr).unwrap(),
        weight: 1,
    }
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
    use crate::repository::{hosts, routes, subsystems, upstreams};
    commit(db, &format!("seed {name}"), |conn| {
        let subsystem = subsystems::create(conn, &subsystem_fields(name))?;
        hosts::add(conn, subsystem, &host(host_pattern))?;
        let group = upstreams::create(
            conn,
            subsystem,
            &upstream_fields("backend", "127.0.0.1:3000"),
        )?;
        routes::create(conn, subsystem, &route_fields("/", group))?;
        Ok(Fixture { subsystem, group })
    })
    .unwrap()
    .value
}
