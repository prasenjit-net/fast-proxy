//! The renewal lease: one row naming the instance that may talk to the CA.
//! Taken and renewed by compare-and-swap so two instances can never both hold it.

use fast_proxy_model::AcmeLease;
use rusqlite::{params, Connection, OptionalExtension};

use crate::StoreError;

/// Takes or renews the lease for `holder` until `expires_at_ms`, when it is
/// free, expired at `now_ms`, or already `holder`'s. Returns whether `holder`
/// holds it now. Run inside a write transaction, so the check and the write
/// are one step.
pub fn try_take(
    conn: &Connection,
    holder: &str,
    now_ms: i64,
    expires_at_ms: i64,
) -> Result<bool, StoreError> {
    let changed = conn.execute(
        "INSERT INTO acme_lease (id, holder, expires_at) VALUES (1, ?1, ?3)
         ON CONFLICT (id) DO UPDATE SET holder = ?1, expires_at = ?3
         WHERE acme_lease.holder = ?1 OR acme_lease.expires_at <= ?2",
        params![holder, now_ms, expires_at_ms],
    )?;
    Ok(changed == 1)
}

/// Gives the lease up if `holder` has it (on shutdown, so another instance
/// need not wait for it to expire). Returns whether it was released.
pub fn release(conn: &Connection, holder: &str) -> Result<bool, StoreError> {
    Ok(conn.execute("DELETE FROM acme_lease WHERE holder = ?1", [holder])? == 1)
}

/// The lease as stored, expired or not.
pub fn get(conn: &Connection) -> Result<Option<AcmeLease>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT holder, expires_at FROM acme_lease WHERE id = 1",
            [],
            |row| {
                Ok(AcmeLease {
                    holder: row.get(0)?,
                    expires_at_ms: row.get(1)?,
                })
            },
        )
        .optional()?)
}
