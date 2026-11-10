//! Admin tokens. Only the SHA-256 of a secret is stored, as opaque bytes: this
//! crate never hashes or verifies. [`list`] and [`find`] return [`TokenRow`],
//! which has no hash; only [`credentials`] returns hashes, for the auth cache.

use fast_proxy_model::{Role, SecretHash, TokenId, TokenName};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::expect_changed;
use crate::StoreError;

/// `last_used_at` is refreshed at most this often, so authenticating does not
/// turn every request into a write.
const LAST_USED_GRANULARITY_MS: i64 = 60_000;

/// A token to store.
pub struct NewToken {
    pub name: TokenName,
    pub role: Role,
    pub secret_hash: SecretHash,
    /// The creating token's name, or `bootstrap` / `cli`.
    pub created_by: String,
    pub expires_at_ms: Option<i64>,
}

/// A token as shown to the API: never the hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRow {
    pub id: TokenId,
    pub name: TokenName,
    pub role: Role,
    pub created_at_ms: i64,
    pub created_by: String,
    pub expires_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
}

/// What the in-memory token set needs to authenticate: the hash included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenCredential {
    pub id: TokenId,
    pub name: TokenName,
    pub role: Role,
    pub secret_hash: SecretHash,
    pub expires_at_ms: Option<i64>,
    pub revoked: bool,
}

const ROW_COLUMNS: &str =
    "id, name, role, created_at, created_by, expires_at, revoked_at, last_used_at";

/// Stores a token created at `now_ms`; a duplicate name is a `Conflict`.
pub fn create(conn: &Connection, token: &NewToken, now_ms: i64) -> Result<TokenId, StoreError> {
    conn.execute(
        "INSERT INTO admin_token (name, role, secret_hash, created_at, created_by, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            token.name.as_str(),
            token.role.as_str(),
            token.secret_hash.as_bytes(),
            now_ms,
            token.created_by,
            token.expires_at_ms
        ],
    )
    .map_err(|err| match StoreError::from(err) {
        StoreError::Conflict(_) => StoreError::Conflict(format!(
            "a token named \"{}\" already exists",
            token.name.as_str()
        )),
        other => other,
    })?;
    Ok(TokenId(conn.last_insert_rowid()))
}

/// Every token, oldest first.
pub fn list(conn: &Connection) -> Result<Vec<TokenRow>, StoreError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ROW_COLUMNS} FROM admin_token ORDER BY id"
    ))?;
    let rows = statement.query_map([], read_row)?;
    rows.map(|row| row?.into_token_row()).collect()
}

/// One token.
pub fn find(conn: &Connection, id: TokenId) -> Result<TokenRow, StoreError> {
    let raw = conn
        .query_row(
            &format!("SELECT {ROW_COLUMNS} FROM admin_token WHERE id = ?1"),
            [id.0],
            read_row,
        )
        .optional()?;
    raw.ok_or_else(|| StoreError::NotFound(format!("token {}", id.0)))?
        .into_token_row()
}

/// Every token with its hash, for rebuilding the authentication cache.
pub fn credentials(conn: &Connection) -> Result<Vec<TokenCredential>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, name, role, secret_hash, expires_at, revoked_at IS NOT NULL
         FROM admin_token ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Vec<u8>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, bool>(5)?,
        ))
    })?;
    rows.map(|row| {
        let (id, name, role, hash, expires_at_ms, revoked) = row?;
        Ok(TokenCredential {
            id: TokenId(id),
            name: parse_stored(TokenName::parse(&name))?,
            role: parse_stored(Role::parse(&role))?,
            secret_hash: SecretHash::from_slice(&hash)
                .ok_or_else(|| StoreError::Internal(format!("token {id} has a corrupt hash")))?,
            expires_at_ms,
            revoked,
        })
    })
    .collect()
}

/// Revokes a token at `now_ms`. Revoking one that is already revoked is a
/// no-op; revoking the last usable admin token is a `Conflict`.
pub fn revoke(conn: &Connection, id: TokenId, now_ms: i64) -> Result<(), StoreError> {
    let token = find(conn, id)?;
    if token.revoked_at_ms.is_some() {
        return Ok(());
    }
    let is_usable_admin = token.role == Role::Admin && !is_expired(&token, now_ms);
    if is_usable_admin && usable_admin_count(conn, now_ms)? <= 1 {
        return Err(StoreError::Conflict(
            "the last usable admin token cannot be revoked".into(),
        ));
    }
    let changed = conn.execute(
        "UPDATE admin_token SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL",
        params![now_ms, id.0],
    )?;
    expect_changed(changed, &format!("token {}", id.0))
}

/// Records a use at `now_ms`, at most once a minute per token.
pub fn mark_used(conn: &Connection, id: TokenId, now_ms: i64) -> Result<(), StoreError> {
    conn.execute(
        "UPDATE admin_token SET last_used_at = ?1
         WHERE id = ?2 AND (last_used_at IS NULL OR last_used_at <= ?3)",
        params![now_ms, id.0, now_ms - LAST_USED_GRANULARITY_MS],
    )?;
    Ok(())
}

/// How many admin tokens are neither revoked nor expired at `now_ms`.
pub fn usable_admin_count(conn: &Connection, now_ms: i64) -> Result<u32, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM admin_token
         WHERE role = 'admin' AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?1)",
        [now_ms],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

fn is_expired(token: &TokenRow, now_ms: i64) -> bool {
    token.expires_at_ms.is_some_and(|expires| expires <= now_ms)
}

/// The columns of one row before the stored text is re-validated.
struct RawRow {
    id: i64,
    name: String,
    role: String,
    created_at_ms: i64,
    created_by: String,
    expires_at_ms: Option<i64>,
    revoked_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
}

fn read_row(row: &Row<'_>) -> rusqlite::Result<RawRow> {
    Ok(RawRow {
        id: row.get(0)?,
        name: row.get(1)?,
        role: row.get(2)?,
        created_at_ms: row.get(3)?,
        created_by: row.get(4)?,
        expires_at_ms: row.get(5)?,
        revoked_at_ms: row.get(6)?,
        last_used_at_ms: row.get(7)?,
    })
}

impl RawRow {
    fn into_token_row(self) -> Result<TokenRow, StoreError> {
        Ok(TokenRow {
            id: TokenId(self.id),
            name: parse_stored(TokenName::parse(&self.name))?,
            role: parse_stored(Role::parse(&self.role))?,
            created_at_ms: self.created_at_ms,
            created_by: self.created_by,
            expires_at_ms: self.expires_at_ms,
            revoked_at_ms: self.revoked_at_ms,
            last_used_at_ms: self.last_used_at_ms,
        })
    }
}

/// Stored text that no longer validates is corruption, not user input.
fn parse_stored<T>(parsed: Result<T, fast_proxy_model::FieldError>) -> Result<T, StoreError> {
    parsed.map_err(|err| StoreError::Internal(format!("stored token is invalid: {err}")))
}
