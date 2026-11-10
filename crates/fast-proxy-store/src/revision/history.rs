use fast_proxy_model::Config;
use rusqlite::{params, Connection, OptionalExtension};

use super::config_json;
use crate::clock::now_ms;
use crate::repository::certificates;
use crate::StoreError;

/// How many revisions are kept; older ones are pruned on every append.
const KEEP_REVISIONS: i64 = 100;

/// A revision without its configuration, for listings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionSummary {
    pub version: u64,
    pub created_at_ms: i64,
    pub summary: String,
}

/// A full revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub version: u64,
    pub created_at_ms: i64,
    pub summary: String,
    pub config: Config,
}

fn to_version(raw: i64) -> u64 {
    u64::try_from(raw).unwrap_or(0)
}

/// The latest revision number, or 0 before any change was ever made.
pub fn current_version(conn: &Connection) -> Result<u64, StoreError> {
    let max: Option<i64> =
        conn.query_row("SELECT MAX(version) FROM revision", [], |row| row.get(0))?;
    Ok(max.map_or(0, to_version))
}

/// Stores `config` as the next revision, prunes old ones, returns the new version.
pub fn append(conn: &Connection, summary: &str, config: &Config) -> Result<u64, StoreError> {
    let version = current_version(conn)? + 1;
    conn.execute(
        "INSERT INTO revision (version, created_at, summary, config) VALUES (?1, ?2, ?3, ?4)",
        params![
            i64::try_from(version).unwrap_or(i64::MAX),
            now_ms(),
            summary,
            config_json::to_json(config, &certificates::names(conn)?)?
        ],
    )?;
    conn.execute(
        "DELETE FROM revision WHERE version <= ?1",
        [i64::try_from(version).unwrap_or(i64::MAX) - KEEP_REVISIONS],
    )?;
    Ok(version)
}

/// Revisions newest first. `before` pages: only versions below it are returned.
pub fn list(
    conn: &Connection,
    limit: u32,
    before: Option<u64>,
) -> Result<Vec<RevisionSummary>, StoreError> {
    let before = before.map_or(i64::MAX, |v| i64::try_from(v).unwrap_or(i64::MAX));
    let mut statement = conn.prepare(
        "SELECT version, created_at, summary FROM revision
         WHERE version < ?1 ORDER BY version DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(params![before, limit], |row| {
        Ok(RevisionSummary {
            version: to_version(row.get(0)?),
            created_at_ms: row.get(1)?,
            summary: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// One revision with its full configuration.
pub fn get(conn: &Connection, version: u64) -> Result<Revision, StoreError> {
    let row: Option<(i64, String, String)> = conn
        .query_row(
            "SELECT created_at, summary, config FROM revision WHERE version = ?1",
            [i64::try_from(version).unwrap_or(i64::MAX)],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (created_at_ms, summary, json) =
        row.ok_or_else(|| StoreError::NotFound(format!("revision {version}")))?;
    Ok(Revision {
        version,
        created_at_ms,
        summary,
        config: config_json::from_json(&json)?,
    })
}

/// The certificates revision `version` references, as `(id, name)`, so a
/// rollback can name one that was deleted since.
pub fn referenced_certificates(
    conn: &Connection,
    version: u64,
) -> Result<Vec<(fast_proxy_model::CertificateId, String)>, StoreError> {
    let json: Option<String> = conn
        .query_row(
            "SELECT config FROM revision WHERE version = ?1",
            [i64::try_from(version).unwrap_or(i64::MAX)],
            |row| row.get(0),
        )
        .optional()?;
    let json = json.ok_or_else(|| StoreError::NotFound(format!("revision {version}")))?;
    config_json::referenced_certificates(&json)
}
