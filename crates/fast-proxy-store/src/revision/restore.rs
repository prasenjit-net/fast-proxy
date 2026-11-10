use std::collections::HashSet;

use fast_proxy_model::Config;
use rusqlite::{params, Connection};

use super::history;
use crate::inputs::{EndpointFields, SubsystemFields, UpstreamFields};
use crate::repository::{ca_bundles, certificates, endpoints, subsystems, upstreams};
use crate::StoreError;

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Replaces the live tables with the configuration stored in `version`,
/// keeping all ids. The caller records the result as a new revision.
///
/// Certificate material is never touched. A revision that references a
/// certificate or CA bundle that no longer exists is refused with a message
/// naming it, and nothing changes.
pub fn restore(conn: &Connection, version: u64) -> Result<Config, StoreError> {
    let config = history::get(conn, version)?.config;
    ensure_references_exist(conn, version, &config)?;
    conn.execute("DELETE FROM subsystem", [])?;
    for subsystem in &config.subsystems {
        let fields = SubsystemFields {
            name: subsystem.name.clone(),
            enabled: subsystem.enabled,
            description: subsystem.description.clone(),
            https: subsystem.https,
        };
        subsystems::insert(conn, Some(subsystem.id), &fields)?;
        for host in &subsystem.hosts {
            conn.execute(
                "INSERT INTO subsystem_host (subsystem_id, pattern) VALUES (?1, ?2)",
                params![subsystem.id.0, host.to_pattern_string()],
            )?;
        }
        for group in &subsystem.upstream_groups {
            let fields = UpstreamFields {
                name: group.name.clone(),
                policy: group.policy.clone(),
            };
            upstreams::insert(conn, Some(group.id), subsystem.id, &fields)?;
            for endpoint in &group.endpoints {
                let fields = EndpointFields {
                    addr: endpoint.addr.clone(),
                    weight: endpoint.weight,
                    state: endpoint.state,
                };
                endpoints::insert(conn, Some(endpoint.id), group.id, &fields, "endpoints")?;
            }
        }
        for route in &subsystem.routes {
            conn.execute(
                "INSERT INTO route (id, subsystem_id, prefix, upstream_group_id, strip_prefix,
                                    upstream_prefix, preserve_host, connect_timeout_ms,
                                    response_timeout_ms, retry_max_attempts,
                                    retry_idempotent_on_5xx, retry_on_timeout, enabled)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                params![
                    route.id.0,
                    subsystem.id.0,
                    route.prefix.as_str(),
                    route.upstream_group_id.0,
                    route.strip_prefix,
                    route.upstream_prefix.as_str(),
                    route.preserve_host,
                    millis(route.connect_timeout),
                    millis(route.response_timeout),
                    route.retry.max_attempts,
                    route.retry.idempotent_on_5xx,
                    route.retry.on_timeout,
                    route.enabled,
                ],
            )?;
        }
    }
    Ok(config)
}

/// Fails with a `Conflict` naming the first certificate or CA bundle that
/// `config` references and the database no longer holds. A certificate counts
/// as present only when both its id and the name recorded in the revision match,
/// because SQLite may reuse an id.
fn ensure_references_exist(
    conn: &Connection,
    version: u64,
    config: &Config,
) -> Result<(), StoreError> {
    let recorded = history::referenced_certificates(conn, version)?;
    let current = certificates::names(conn)?;
    for (id, name) in recorded {
        if current.get(&id.0) != Some(&name) {
            return Err(StoreError::Conflict(format!(
                "cannot roll back: certificate \"{name}\" no longer exists; upload it again first"
            )));
        }
    }
    let bundles: HashSet<String> = ca_bundles::ids_by_name(conn)?.into_keys().collect();
    for group in config
        .subsystems
        .iter()
        .flat_map(|subsystem| &subsystem.upstream_groups)
    {
        let ca = &group.policy.tls.ca;
        if !ca.is_empty() && !bundles.contains(ca) {
            return Err(StoreError::Conflict(format!(
                "cannot roll back: CA bundle \"{ca}\" no longer exists; upload it again first"
            )));
        }
    }
    Ok(())
}
