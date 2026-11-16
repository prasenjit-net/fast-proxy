//! Upgrades a database written by an earlier release to the current schema.
//!
//! `schema.sql` creates a **new** database at the current version. A database from
//! an older release runs every step after its version, in order and in one
//! transaction, so an upgrade either completes or leaves the file as it was. Steps
//! are additive (AGENTS.md, "Compatibility and migration"): new tables, and new
//! columns that are nullable or have a default. Before the first step a copy of
//! the file is kept next to it ([`backup`]).

mod backup;
#[cfg(test)]
mod schema_shape;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use rusqlite::{Connection, TransactionBehavior};

use crate::database::SCHEMA_VERSION;
use crate::StoreError;

/// The oldest schema this build upgrades from: the one v0.1.1 shipped.
pub(crate) const OLDEST_UPGRADABLE: i64 = 5;

/// One upgrade: the SQL that takes the version before `to` to `to`.
struct Step {
    to: i64,
    sql: &'static str,
}

/// Every step from [`OLDEST_UPGRADABLE`] to [`SCHEMA_VERSION`], one per version.
const STEPS: &[Step] = &[Step {
    to: 6,
    sql: include_str!("v6_acme.sql"),
}];

/// What opening a database did to its schema, so startup can report it once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaUpgrade {
    /// The version the file had.
    pub from: i64,
    /// The version it has now.
    pub to: i64,
    /// The copy taken before the upgrade (none for an in-memory database).
    pub backup: Option<PathBuf>,
}

/// Whether `found` is a version this build can upgrade (older, but not too old).
pub(crate) fn is_upgradable(found: i64) -> bool {
    (OLDEST_UPGRADABLE..SCHEMA_VERSION).contains(&found)
}

/// Upgrades the database to [`SCHEMA_VERSION`], first copying the file at `path`
/// when there is one. Another process may be doing the same on the same file:
/// the version is read again under the write lock, and a database it already
/// upgraded is left alone.
pub(crate) fn upgrade(
    conn: &mut Connection,
    path: Option<&Path>,
    found: i64,
) -> Result<SchemaUpgrade, StoreError> {
    let backup = path
        .map(|path| backup::copy_before_upgrade(conn, path, SCHEMA_VERSION))
        .transpose()?;
    apply_steps(conn)?;
    Ok(SchemaUpgrade {
        from: found,
        to: SCHEMA_VERSION,
        backup,
    })
}

/// Runs the missing steps and sets the new version, all in one transaction.
fn apply_steps(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    if !is_upgradable(found) {
        return Err(StoreError::IncompatibleSchema {
            found,
            expected: SCHEMA_VERSION,
        });
    }
    for step in STEPS.iter().filter(|step| step.to > found) {
        tx.execute_batch(step.sql)?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}
