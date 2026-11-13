//! Sampled requests, written by every instance sharing the database and read
//! by any of them. Samples are telemetry, not configuration: nothing here
//! records a revision, and the store knows nothing of what a sample means
//! beyond its short columns (the redacted `detail` is opaque JSON).

use fast_proxy_model::{Trigger, TriggerSet};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};

use crate::StoreError;

/// The most rows one [`list`] call returns.
pub const MAX_PAGE: u32 = 500;

/// A sample to store. The caller has redacted it already.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSample {
    /// When the response finished, unix milliseconds.
    pub at_ms: i64,
    pub instance: String,
    pub request_id: String,
    /// Every trigger that matched; at least one.
    pub triggers: TriggerSet,
    pub subsystem: String,
    pub route: String,
    pub client: String,
    pub method: String,
    pub status: u16,
    pub total_ms: u32,
    /// The redacted sample as JSON.
    pub detail: String,
}

/// A stored sample without its detail: what lists and filters read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleSummary {
    pub id: i64,
    pub at_ms: i64,
    pub instance: String,
    pub request_id: String,
    pub triggers: TriggerSet,
    pub primary: Trigger,
    pub subsystem: String,
    pub route: String,
    pub client: String,
    pub method: String,
    pub status: u16,
    pub total_ms: u32,
    pub size_bytes: u64,
}

/// A stored sample with its detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleRecord {
    pub summary: SampleSummary,
    pub detail: String,
}

/// What [`list`] selects. Empty fields do not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SampleFilter {
    /// Match samples that have any of these triggers.
    pub triggers: TriggerSet,
    /// 1 to 5: the first digit of the status.
    pub status_class: Option<u8>,
    pub subsystem: Option<String>,
    pub route: Option<String>,
    pub instance: Option<String>,
    pub client: Option<String>,
    /// At least this many milliseconds in total.
    pub min_ms: Option<u32>,
    pub since_ms: Option<i64>,
    /// The cursor: only samples with a smaller id (older than this one).
    pub before_id: Option<i64>,
    /// At most this many (default and ceiling [`MAX_PAGE`]).
    pub limit: Option<u32>,
}

/// How much the table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub rows: u64,
    pub bytes: u64,
    pub oldest_at_ms: Option<i64>,
}

/// How far over its limits the table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Excess {
    pub rows: u64,
    pub bytes: u64,
}

impl Excess {
    pub fn is_none(self) -> bool {
        self.rows == 0 && self.bytes == 0
    }
}

impl Usage {
    /// How much of this usage is over `max_rows` and `max_bytes`.
    pub fn excess(self, max_rows: u64, max_bytes: u64) -> Excess {
        Excess {
            rows: self.rows.saturating_sub(max_rows),
            bytes: self.bytes.saturating_sub(max_bytes),
        }
    }
}

/// What a delete step removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deleted {
    pub rows: u64,
    pub bytes: u64,
}

/// Stores `samples` all-or-nothing and returns how many were stored. A sample
/// without a trigger is refused (`Invalid`, nothing stored). Safe to call
/// inside a transaction.
pub fn insert_batch(conn: &Connection, samples: &[NewSample]) -> Result<usize, StoreError> {
    if samples.iter().any(|sample| sample.triggers.is_empty()) {
        return Err(StoreError::invalid_field(
            "triggers",
            "a sample needs at least one trigger",
        ));
    }
    conn.execute_batch("SAVEPOINT sample_batch")?;
    match insert_all(conn, samples) {
        Ok(count) => {
            conn.execute_batch("RELEASE sample_batch")?;
            Ok(count)
        }
        Err(err) => {
            // SQLite rolls the whole transaction back by itself on some errors
            // (a full disk), taking the savepoint with it; the error to report
            // is the original one, not the failure to undo it.
            let _ = conn.execute_batch("ROLLBACK TO sample_batch; RELEASE sample_batch");
            Err(err)
        }
    }
}

fn insert_all(conn: &Connection, samples: &[NewSample]) -> Result<usize, StoreError> {
    let mut insert = conn.prepare_cached(
        "INSERT INTO sample
           (at, instance, request_id, triggers, primary_trigger, subsystem, route, client,
            method, status, total_ms, size_bytes, detail)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
    )?;
    for sample in samples {
        let primary = sample.triggers.primary().map_or("", Trigger::as_str);
        insert.execute(params![
            sample.at_ms,
            sample.instance,
            sample.request_id,
            sample.triggers.bits(),
            primary,
            sample.subsystem,
            sample.route,
            sample.client,
            sample.method,
            sample.status,
            sample.total_ms,
            stored_size(sample),
            sample.detail,
        ])?;
    }
    Ok(samples.len())
}

/// The bytes a sample counts for against the byte limit: its detail and its
/// short text columns.
fn stored_size(sample: &NewSample) -> i64 {
    let bytes = sample.detail.len()
        + sample.instance.len()
        + sample.request_id.len()
        + sample.subsystem.len()
        + sample.route.len()
        + sample.client.len()
        + sample.method.len();
    i64::try_from(bytes).unwrap_or(i64::MAX)
}

/// Samples matching `filter`, newest first, with their detail: for export.
/// Lists that do not need the detail use [`list`], which never reads it.
pub fn list_records(
    conn: &Connection,
    filter: &SampleFilter,
) -> Result<Vec<SampleRecord>, StoreError> {
    let (clauses, mut values) = where_clause(filter);
    let limit = filter.limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE);
    values.push(Value::from(i64::from(limit)));
    let sql = format!(
        "SELECT id, at, instance, request_id, triggers, primary_trigger, subsystem, route, client,
                method, status, total_ms, size_bytes, detail
         FROM sample {clauses} ORDER BY id DESC LIMIT ?{}",
        values.len()
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| {
        Ok((read_summary(row)?, row.get::<_, String>(13)?))
    })?;
    rows.map(|row| {
        let (summary, detail) = row?;
        Ok(SampleRecord {
            summary: summary?,
            detail,
        })
    })
    .collect()
}

/// Samples matching `filter`, newest first. Never reads the detail.
pub fn list(conn: &Connection, filter: &SampleFilter) -> Result<Vec<SampleSummary>, StoreError> {
    let (clauses, mut values) = where_clause(filter);
    let limit = filter.limit.unwrap_or(MAX_PAGE).clamp(1, MAX_PAGE);
    values.push(Value::from(i64::from(limit)));
    let sql = format!(
        "SELECT id, at, instance, request_id, triggers, primary_trigger, subsystem, route, client,
                method, status, total_ms, size_bytes
         FROM sample {clauses} ORDER BY id DESC LIMIT ?{}",
        values.len()
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(values), read_summary)?;
    rows.map(|row| row?).collect()
}

fn where_clause(filter: &SampleFilter) -> (String, Vec<Value>) {
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    let mut add = |clause: &str, value: Value| {
        values.push(value);
        clauses.push(clause.replace('?', &format!("?{}", values.len())));
    };
    if !filter.triggers.is_empty() {
        add(
            "(triggers & ?) <> 0",
            Value::from(i64::from(filter.triggers.bits())),
        );
    }
    if let Some(class) = filter.status_class {
        add("status >= ?", Value::from(i64::from(class) * 100));
        add("status < ?", Value::from(i64::from(class) * 100 + 100));
    }
    for (column, value) in [
        ("subsystem", &filter.subsystem),
        ("route", &filter.route),
        ("instance", &filter.instance),
        ("client", &filter.client),
    ] {
        if let Some(value) = value {
            add(&format!("{column} = ?"), Value::from(value.clone()));
        }
    }
    if let Some(min_ms) = filter.min_ms {
        add("total_ms >= ?", Value::from(i64::from(min_ms)));
    }
    if let Some(since) = filter.since_ms {
        add("at >= ?", Value::from(since));
    }
    if let Some(before) = filter.before_id {
        add("id < ?", Value::from(before));
    }
    if clauses.is_empty() {
        return (String::new(), values);
    }
    (format!("WHERE {}", clauses.join(" AND ")), values)
}

/// One sample with its detail; `NotFound` when it is gone (pruned, cleared).
pub fn get(conn: &Connection, id: i64) -> Result<SampleRecord, StoreError> {
    let found = conn
        .query_row(
            "SELECT id, at, instance, request_id, triggers, primary_trigger, subsystem, route,
                    client, method, status, total_ms, size_bytes, detail
             FROM sample WHERE id = ?1",
            [id],
            |row| Ok((read_summary(row)?, row.get::<_, String>(13)?)),
        )
        .optional()?;
    let (summary, detail) = found.ok_or_else(|| StoreError::NotFound(format!("sample {id}")))?;
    Ok(SampleRecord {
        summary: summary?,
        detail,
    })
}

/// How many rows the table holds, how many bytes they count for, and the age
/// of the oldest.
pub fn usage(conn: &Connection) -> Result<Usage, StoreError> {
    // Three queries, each answered from one narrow index, rather than one that
    // would have to read every row.
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM sample", [], |row| row.get(0))?;
    let bytes: i64 = conn.query_row(
        "SELECT COALESCE(SUM(size_bytes), 0) FROM sample",
        [],
        |row| row.get(0),
    )?;
    let oldest: Option<i64> = conn.query_row("SELECT MIN(at) FROM sample", [], |row| row.get(0))?;
    Ok(Usage {
        rows: u64::try_from(rows).unwrap_or(0),
        bytes: u64::try_from(bytes).unwrap_or(0),
        oldest_at_ms: oldest,
    })
}

/// Deletes up to `batch` of the samples that finished before `cutoff_ms`,
/// oldest first, through the time index. Returns how many; call again until it
/// returns 0.
pub fn delete_older_than(
    conn: &Connection,
    cutoff_ms: i64,
    batch: u32,
) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "DELETE FROM sample WHERE id IN
           (SELECT id FROM sample WHERE at < ?1 ORDER BY at LIMIT ?2)",
        params![cutoff_ms, batch],
    )?)
}

/// Deletes the oldest samples, at most `batch`, stopping as soon as at least
/// `excess.rows` rows and `excess.bytes` bytes are gone. It reads only the
/// `batch` oldest rows, so it is cheap however large the table is; the caller
/// measures the table once (with [`usage`]) and subtracts what each call
/// returns.
pub fn delete_oldest_covering(
    conn: &Connection,
    excess: Excess,
    batch: u32,
) -> Result<Deleted, StoreError> {
    let mut statement = conn.prepare("SELECT id, size_bytes FROM sample ORDER BY id LIMIT ?1")?;
    let oldest = statement.query_map([batch], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    })?;
    let (mut rows, mut bytes, mut last_id) = (0u64, 0u64, None);
    for row in oldest {
        let (id, size) = row?;
        rows += 1;
        bytes += u64::try_from(size).unwrap_or(0);
        last_id = Some(id);
        if rows >= excess.rows && bytes >= excess.bytes {
            break;
        }
    }
    let Some(last_id) = last_id else {
        return Ok(Deleted { rows: 0, bytes: 0 });
    };
    conn.execute("DELETE FROM sample WHERE id <= ?1", [last_id])?;
    Ok(Deleted { rows, bytes })
}

/// Deletes up to `batch` of the oldest samples whatever their age (clearing
/// the table in short statements); returns how many.
pub fn clear_step(conn: &Connection, batch: u32) -> Result<usize, StoreError> {
    Ok(conn.execute(
        "DELETE FROM sample WHERE id IN (SELECT id FROM sample ORDER BY id LIMIT ?1)",
        [batch],
    )?)
}

fn read_summary(row: &Row<'_>) -> rusqlite::Result<Result<SampleSummary, StoreError>> {
    let id: i64 = row.get(0)?;
    let triggers: u32 = row.get(4)?;
    let primary: String = row.get(5)?;
    let status: u32 = row.get(10)?;
    let size: i64 = row.get(12)?;
    let corrupt = |why: &str| StoreError::Internal(format!("sample {id}: {why}"));
    let parsed = (
        TriggerSet::from_bits(triggers),
        Trigger::parse("primary", &primary),
        u16::try_from(status),
    );
    let (Some(triggers), Ok(primary), Ok(status)) = parsed else {
        return Ok(Err(corrupt("unreadable triggers or status")));
    };
    Ok(Ok(SampleSummary {
        id,
        at_ms: row.get(1)?,
        instance: row.get(2)?,
        request_id: row.get(3)?,
        triggers,
        primary,
        subsystem: row.get(6)?,
        route: row.get(7)?,
        client: row.get(8)?,
        method: row.get(9)?,
        status,
        total_ms: row.get(11)?,
        size_bytes: u64::try_from(size).unwrap_or(0),
    }))
}
