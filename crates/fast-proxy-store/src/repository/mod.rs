//! Row-level operations, one file per resource. Every function takes a
//! `&Connection` (a `Transaction` derefs to one), so the same code serves
//! reads and the transactional write path.

pub mod endpoints;
pub mod hosts;
pub mod load;
mod policy_columns;
pub mod routes;
pub mod subsystems;
pub mod upstreams;

use fast_proxy_model::SubsystemId;
use rusqlite::{params, Connection, OptionalExtension};

use crate::clock::now_ms;
use crate::StoreError;

/// Fails with `NotFound(what)` when an UPDATE or DELETE touched no row.
fn expect_changed(changed: usize, what: &str) -> Result<(), StoreError> {
    if changed == 0 {
        return Err(StoreError::NotFound(what.to_string()));
    }
    Ok(())
}

/// Fails with `NotFound` unless the subsystem exists.
fn ensure_subsystem(conn: &Connection, id: SubsystemId) -> Result<(), StoreError> {
    let found = conn
        .query_row("SELECT 1 FROM subsystem WHERE id = ?1", [id.0], |_| Ok(()))
        .optional()?;
    found.ok_or_else(|| StoreError::NotFound(format!("subsystem {}", id.0)))
}

/// Records that something under the subsystem changed.
fn touch(conn: &Connection, id: SubsystemId) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE subsystem SET updated_at = ?1 WHERE id = ?2",
        params![now_ms(), id.0],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
