//! The audit log: append-only from the application, read newest first,
//! exported oldest first, and pruned by age and count. Entries hold names and
//! ids only; callers never pass a body, secret, key or PEM.

use rusqlite::{params, params_from_iter, types::Value, Connection};

use crate::StoreError;

/// Largest `detail`, in bytes.
pub const MAX_DETAIL_BYTES: usize = 2048;

/// How an audited operation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Ok,
    Denied,
    Failed,
}

impl AuditOutcome {
    /// The stored and API spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Denied => "denied",
            Self::Failed => "failed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "ok" => Some(Self::Ok),
            "denied" => Some(Self::Denied),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// An entry to append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAuditEntry {
    pub at_ms: i64,
    pub instance: String,
    /// A token name, `anonymous` (failed authentication) or `system`.
    pub actor: String,
    pub role: String,
    pub source: String,
    pub request_id: String,
    /// A stable name such as `subsystem.update`.
    pub action: String,
    pub target: String,
    pub outcome: AuditOutcome,
    /// Short JSON: ids, the revision number, the error code.
    pub detail: String,
    pub revision: Option<u64>,
}

/// A stored entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: i64,
    pub entry: NewAuditEntry,
}

/// Filters for [`list`]; every set field must match.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    pub actor: Option<String>,
    pub action: Option<String>,
    pub outcome: Option<AuditOutcome>,
    pub since_ms: Option<i64>,
}

const COLUMNS: &str =
    "id, at, instance, actor, role, source, request_id, action, target, outcome, detail, revision";

/// Appends an entry (inside the caller's transaction when it has one); a
/// `detail` over 2 KiB is refused.
pub fn append(conn: &Connection, entry: &NewAuditEntry) -> Result<i64, StoreError> {
    if entry.detail.len() > MAX_DETAIL_BYTES {
        return Err(StoreError::invalid_field(
            "detail",
            "must be at most 2048 bytes",
        ));
    }
    conn.execute(
        "INSERT INTO audit_log
           (at, instance, actor, role, source, request_id, action, target, outcome, detail, revision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            entry.at_ms,
            entry.instance,
            entry.actor,
            entry.role,
            entry.source,
            entry.request_id,
            entry.action,
            entry.target,
            entry.outcome.as_str(),
            entry.detail,
            entry.revision.map(|v| i64::try_from(v).unwrap_or(i64::MAX)),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Entries newest first. `before` pages: only ids below it are returned, which
/// stays stable while new entries arrive.
pub fn list(
    conn: &Connection,
    query: &AuditQuery,
    before: Option<i64>,
    limit: u32,
) -> Result<Vec<AuditEntry>, StoreError> {
    let (filter, mut values) = filter_sql(query);
    let sql =
        format!("SELECT {COLUMNS} FROM audit_log WHERE id < ? {filter} ORDER BY id DESC LIMIT ?");
    values.insert(0, Value::Integer(before.unwrap_or(i64::MAX)));
    values.push(Value::Integer(i64::from(limit)));
    run_query(conn, &sql, values)
}

/// Entries oldest first, for the export: those after `after_id` (0 to start)
/// and not older than `since_ms`, at most `limit` per call.
pub fn export(
    conn: &Connection,
    since_ms: Option<i64>,
    after_id: i64,
    limit: u32,
) -> Result<Vec<AuditEntry>, StoreError> {
    let query = AuditQuery {
        since_ms,
        ..AuditQuery::default()
    };
    let (filter, mut values) = filter_sql(&query);
    let sql =
        format!("SELECT {COLUMNS} FROM audit_log WHERE id > ? {filter} ORDER BY id ASC LIMIT ?");
    values.insert(0, Value::Integer(after_id));
    values.push(Value::Integer(i64::from(limit)));
    run_query(conn, &sql, values)
}

/// Deletes up to `batch` entries older than `cutoff_ms`, then up to `batch`
/// of those beyond the newest `max_rows`; returns how many went. Callers loop
/// until it returns 0, so no single call holds the writer lock for long. The
/// newest `max_rows` entries are never removed by the count rule.
pub fn prune(
    conn: &Connection,
    cutoff_ms: i64,
    max_rows: u32,
    batch: u32,
) -> Result<usize, StoreError> {
    let by_age = conn.execute(
        "DELETE FROM audit_log WHERE id IN
           (SELECT id FROM audit_log WHERE at < ?1 ORDER BY id LIMIT ?2)",
        params![cutoff_ms, batch],
    )?;
    let by_count = conn.execute(
        "DELETE FROM audit_log WHERE id IN
           (SELECT id FROM audit_log
            WHERE id <= COALESCE((SELECT id FROM audit_log ORDER BY id DESC LIMIT 1 OFFSET ?1), 0)
            ORDER BY id LIMIT ?2)",
        params![max_rows, batch],
    )?;
    Ok(by_age + by_count)
}

fn filter_sql(query: &AuditQuery) -> (String, Vec<Value>) {
    let mut sql = String::new();
    let mut values = Vec::new();
    if let Some(actor) = &query.actor {
        sql.push_str(" AND actor = ?");
        values.push(Value::Text(actor.clone()));
    }
    if let Some(action) = &query.action {
        sql.push_str(" AND action = ?");
        values.push(Value::Text(action.clone()));
    }
    if let Some(outcome) = query.outcome {
        sql.push_str(" AND outcome = ?");
        values.push(Value::Text(outcome.as_str().to_string()));
    }
    if let Some(since) = query.since_ms {
        sql.push_str(" AND at >= ?");
        values.push(Value::Integer(since));
    }
    (sql, values)
}

/// Runs a query whose placeholders are all positional `?`.
fn run_query(
    conn: &Connection,
    sql: &str,
    values: Vec<Value>,
) -> Result<Vec<AuditEntry>, StoreError> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, String>(7)?,
            row.get::<_, String>(8)?,
            row.get::<_, String>(9)?,
            row.get::<_, String>(10)?,
            row.get::<_, Option<i64>>(11)?,
        ))
    })?;
    rows.map(|row| {
        let (
            id,
            at_ms,
            instance,
            actor,
            role,
            source,
            request_id,
            action,
            target,
            outcome,
            detail,
            revision,
        ) = row?;
        let outcome = AuditOutcome::parse(&outcome)
            .ok_or_else(|| StoreError::Internal(format!("audit entry {id} has a bad outcome")))?;
        Ok(AuditEntry {
            id,
            entry: NewAuditEntry {
                at_ms,
                instance,
                actor,
                role,
                source,
                request_id,
                action,
                target,
                outcome,
                detail,
                revision: revision.and_then(|v| u64::try_from(v).ok()),
            },
        })
    })
    .collect()
}
