//! The single `vault` row: an encrypted check value that proves the master key
//! is the one the stored keys were sealed with. Opaque here, like every key.

use rusqlite::{params, Connection, OptionalExtension};

use crate::StoreError;

/// The sealed check value.
#[derive(Clone, PartialEq, Eq)]
pub struct VaultCheck {
    pub nonce: Vec<u8>,
    pub blob: Vec<u8>,
}

impl std::fmt::Debug for VaultCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VaultCheck").finish_non_exhaustive()
    }
}

/// The stored check value, if the vault was ever created.
pub fn get(conn: &Connection) -> Result<Option<VaultCheck>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT check_nonce, check_blob FROM vault WHERE id = 1",
            [],
            |row| {
                Ok(VaultCheck {
                    nonce: row.get(0)?,
                    blob: row.get(1)?,
                })
            },
        )
        .optional()?)
}

/// Stores the check value, replacing any earlier one.
pub fn set(conn: &Connection, check: &VaultCheck) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO vault (id, check_nonce, check_blob) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET check_nonce = ?1, check_blob = ?2",
        params![check.nonce, check.blob],
    )?;
    Ok(())
}
