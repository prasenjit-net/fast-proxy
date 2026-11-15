//! Pending HTTP-01 challenges. They are compiled into the routing snapshot so
//! every instance answers them; they are written with a summary-only revision
//! so other instances pick them up through the sync poll.

use fast_proxy_model::{AcmeChallenge, AcmeToken, SubsystemId};
use rusqlite::{params, Connection};

use super::super::ensure_subsystem;
use crate::StoreError;

/// Adds or replaces challenges (keyed by token). A missing subsystem is `NotFound`.
pub fn put(conn: &Connection, challenges: &[AcmeChallenge]) -> Result<(), StoreError> {
    for challenge in challenges {
        ensure_subsystem(conn, challenge.subsystem_id)?;
        let names = serde_json::to_string(&challenge.names)
            .map_err(|err| StoreError::Internal(err.to_string()))?;
        conn.execute(
            "INSERT INTO acme_challenge (token, subsystem_id, names, key_auth, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (token) DO UPDATE SET
               subsystem_id = ?2, names = ?3, key_auth = ?4, expires_at = ?5",
            params![
                challenge.token.as_str(),
                challenge.subsystem_id.0,
                names,
                challenge.key_authorization,
                challenge.expires_at_ms
            ],
        )?;
    }
    Ok(())
}

/// Removes every challenge of `subsystem`; returns how many there were.
pub fn clear_subsystem(conn: &Connection, subsystem: SubsystemId) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "DELETE FROM acme_challenge WHERE subsystem_id = ?1",
        [subsystem.0],
    )?)
}

/// Removes challenges that expired at or before `now_ms`; returns how many.
pub fn clear_expired(conn: &Connection, now_ms: i64) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "DELETE FROM acme_challenge WHERE expires_at <= ?1",
        [now_ms],
    )?)
}

/// Every stored challenge, by token. Expired ones are included: the caller
/// (compile) decides with its own clock.
pub fn list(conn: &Connection) -> Result<Vec<AcmeChallenge>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT token, subsystem_id, names, key_auth, expires_at
         FROM acme_challenge ORDER BY token",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    rows.map(|row| {
        let (token, subsystem, names, key_auth, expires_at) = row?;
        let names: Vec<String> = serde_json::from_str(&names).map_err(corrupt)?;
        AcmeToken::parse(&token)
            .and_then(|token| {
                AcmeChallenge::new(token, SubsystemId(subsystem), names, key_auth, expires_at)
            })
            .map_err(corrupt)
    })
    .collect()
}

fn corrupt(err: impl std::fmt::Display) -> StoreError {
    StoreError::Internal(format!("stored acme challenge is invalid: {err}"))
}
