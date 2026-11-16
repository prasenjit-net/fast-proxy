//! The per-subsystem ACME status, written without a revision and read by every
//! instance's admin plane. A row goes away with its subsystem.

use fast_proxy_model::{
    AcmeState, AcmeStatus, CertificateId, SkipReason, SkippedName, SubsystemId,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::super::ensure_subsystem;
use crate::clock::now_ms;
use crate::StoreError;

const SELECT_LIST: &str = "subsystem_id, state, eligible_names, skipped_names, certificate_id, \
     next_attempt_at, failures, window_failures, failure_window_start, last_error, \
     last_attempt_at, last_success_at, instance";

#[derive(Serialize, Deserialize)]
struct SkippedJson {
    name: String,
    reason: String,
}

/// Stores `status`, replacing the subsystem's earlier row. A missing
/// subsystem is `NotFound`.
pub fn put(conn: &Connection, status: &AcmeStatus) -> Result<(), StoreError> {
    ensure_subsystem(conn, status.subsystem_id)?;
    let eligible = to_json(&status.eligible_names)?;
    let skipped = to_json(
        &status
            .skipped_names
            .iter()
            .map(|skipped| SkippedJson {
                name: skipped.name.clone(),
                reason: skipped.reason.as_str().to_string(),
            })
            .collect::<Vec<_>>(),
    )?;
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO acme_status ({SELECT_LIST}, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
        ),
        params![
            status.subsystem_id.0,
            status.state.as_str(),
            eligible,
            skipped,
            status.certificate_id.map(|id| id.0),
            status.next_attempt_at_ms,
            status.failures,
            status.window_failures,
            status.failure_window_start_ms,
            status.last_error,
            status.last_attempt_at_ms,
            status.last_success_at_ms,
            status.instance,
            now_ms()
        ],
    )?;
    Ok(())
}

/// The status of one subsystem, if one was ever written.
pub fn get(conn: &Connection, subsystem: SubsystemId) -> Result<Option<AcmeStatus>, StoreError> {
    conn.query_row(
        &format!("SELECT {SELECT_LIST} FROM acme_status WHERE subsystem_id = ?1"),
        [subsystem.0],
        read,
    )
    .optional()?
    .transpose()
}

/// Every stored status, by subsystem id.
pub fn list(conn: &Connection) -> Result<Vec<AcmeStatus>, StoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {SELECT_LIST} FROM acme_status ORDER BY subsystem_id"
    ))?;
    let rows = statement.query_map([], read)?;
    rows.map(|row| row?).collect()
}

/// Removes the status of a subsystem that no longer uses ACME.
pub fn delete(conn: &Connection, subsystem: SubsystemId) -> Result<bool, StoreError> {
    Ok(conn.execute(
        "DELETE FROM acme_status WHERE subsystem_id = ?1",
        [subsystem.0],
    )? == 1)
}

fn read(row: &Row<'_>) -> rusqlite::Result<Result<AcmeStatus, StoreError>> {
    let state: String = row.get(1)?;
    let eligible: String = row.get(2)?;
    let skipped: String = row.get(3)?;
    let failures: i64 = row.get(6)?;
    let window_failures: i64 = row.get(7)?;
    let mut status = AcmeStatus::new(SubsystemId(row.get(0)?));
    status.certificate_id = row.get::<_, Option<i64>>(4)?.map(CertificateId);
    status.next_attempt_at_ms = row.get(5)?;
    status.failures = u32::try_from(failures).unwrap_or(u32::MAX);
    status.window_failures = u32::try_from(window_failures).unwrap_or(u32::MAX);
    status.failure_window_start_ms = row.get(8)?;
    status.last_error = row.get(9)?;
    status.last_attempt_at_ms = row.get(10)?;
    status.last_success_at_ms = row.get(11)?;
    status.instance = row.get(12)?;
    let decoded = (|| -> Result<AcmeStatus, StoreError> {
        status.state = AcmeState::parse(&state).map_err(corrupt)?;
        status.eligible_names = serde_json::from_str(&eligible).map_err(corrupt)?;
        status.skipped_names = serde_json::from_str::<Vec<SkippedJson>>(&skipped)
            .map_err(corrupt)?
            .into_iter()
            .map(|json| {
                SkipReason::parse(&json.reason).map(|reason| SkippedName {
                    name: json.name,
                    reason,
                })
            })
            .collect::<Result<_, _>>()
            .map_err(corrupt)?;
        Ok(status)
    })();
    Ok(decoded)
}

fn to_json(value: &impl Serialize) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|err| StoreError::Internal(err.to_string()))
}

fn corrupt(err: impl std::fmt::Display) -> StoreError {
    StoreError::Internal(format!("stored acme status is invalid: {err}"))
}
