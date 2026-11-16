//! ACME accounts, one per directory URL. The credentials (account URL and
//! private key) arrive sealed by the vault and are stored as given.

use rusqlite::{params, Connection, OptionalExtension};

use crate::clock::now_ms;
use crate::StoreError;

/// A stored account. `Debug` never shows the sealed credentials.
#[derive(Clone, PartialEq, Eq)]
pub struct AcmeAccountRow {
    pub directory: String,
    pub email: String,
    pub credentials_enc: Vec<u8>,
    pub credentials_nonce: Vec<u8>,
}

impl std::fmt::Debug for AcmeAccountRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcmeAccountRow")
            .field("directory", &self.directory)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

/// The account registered with `directory`, if any.
pub fn get(conn: &Connection, directory: &str) -> Result<Option<AcmeAccountRow>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT directory, email, credentials_enc, credentials_nonce
             FROM acme_account WHERE directory = ?1",
            [directory],
            |row| {
                Ok(AcmeAccountRow {
                    directory: row.get(0)?,
                    email: row.get(1)?,
                    credentials_enc: row.get(2)?,
                    credentials_nonce: row.get(3)?,
                })
            },
        )
        .optional()?)
}

/// Stores the account for its directory, replacing an earlier one.
pub fn put(conn: &Connection, account: &AcmeAccountRow) -> Result<(), StoreError> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO acme_account
           (directory, email, credentials_enc, credentials_nonce, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT (directory) DO UPDATE SET
           email = ?2, credentials_enc = ?3, credentials_nonce = ?4, updated_at = ?5",
        params![
            account.directory,
            account.email,
            account.credentials_enc,
            account.credentials_nonce,
            now
        ],
    )?;
    Ok(())
}
