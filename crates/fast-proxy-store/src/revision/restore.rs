use fast_proxy_model::Config;
use rusqlite::{params, Connection};

use super::history;
use crate::clock::now_ms;
use crate::inputs::{EndpointFields, UpstreamFields};
use crate::repository::{endpoints, upstreams};
use crate::StoreError;

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

/// Replaces the live tables with the configuration stored in `version`,
/// keeping all ids. The caller records the result as a new revision.
pub fn restore(conn: &Connection, version: u64) -> Result<Config, StoreError> {
    let config = history::get(conn, version)?.config;
    conn.execute("DELETE FROM subsystem", [])?;
    let now = now_ms();
    for subsystem in &config.subsystems {
        conn.execute(
            "INSERT INTO subsystem (id, name, enabled, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                subsystem.id.0,
                subsystem.name.as_str(),
                subsystem.enabled,
                subsystem.description,
                now
            ],
        )?;
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
