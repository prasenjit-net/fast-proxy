//! What the admin plane reads: lists, one sample, the export, the table's size
//! and clearing it. Samples of every instance are in one table, so any
//! instance shows them all. The calls block on SQLite; async callers run them
//! on a blocking thread.

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use fast_proxy_sampling::{Sample, SampleQuery};
use fast_proxy_store::repository::samples::{self, SampleFilter, SampleSummary, Usage};
use fast_proxy_store::{Database, StoreError};

/// Rows fetched per page while exporting.
const EXPORT_PAGE: u32 = 200;
/// Rows deleted per statement while clearing.
const CLEAR_BATCH: u32 = 500;
const CLEAR_PAUSE: Duration = Duration::from_millis(2);

/// Reads (and clears) the stored samples.
pub struct SampleReader {
    database: Mutex<Database>,
}

impl SampleReader {
    /// Opens the reader's own connection to the database at `path`.
    pub fn open(path: &Path, busy_timeout: Duration) -> Result<Self, StoreError> {
        Ok(Self::from_database(Database::open_secondary(
            path,
            busy_timeout,
        )?))
    }

    pub fn from_database(database: Database) -> Self {
        Self {
            database: Mutex::new(database),
        }
    }

    fn database(&self) -> std::sync::MutexGuard<'_, Database> {
        self.database.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The samples matching `query`, newest first, without their detail.
    pub fn list(&self, query: &SampleQuery) -> Result<Vec<SampleSummary>, StoreError> {
        let filter = filter_of(query)?;
        self.database().read(|conn| samples::list(conn, &filter))
    }

    /// One sample with everything in it; `NotFound` once it has been pruned.
    pub fn get(&self, id: i64) -> Result<Sample, StoreError> {
        let record = self.database().read(|conn| samples::get(conn, id))?;
        sample_of(record.summary.id, &record.detail)
    }

    /// How much the table holds.
    pub fn usage(&self) -> Result<Usage, StoreError> {
        self.database().read(samples::usage)
    }

    /// Hands every sample matching `query` to `visit`, newest first, a page at
    /// a time (the query's own limit is ignored). Stops at the first error
    /// `visit` returns.
    pub fn export_each<E>(
        &self,
        query: &SampleQuery,
        mut visit: impl FnMut(&Sample) -> Result<(), E>,
    ) -> Result<Result<(), E>, StoreError> {
        let mut filter = filter_of(query)?;
        filter.limit = Some(EXPORT_PAGE);
        loop {
            let page = self
                .database()
                .read(|conn| samples::list_records(conn, &filter))?;
            let Some(last) = page.last() else {
                return Ok(Ok(()));
            };
            filter.before_id = Some(last.summary.id);
            for record in &page {
                let sample = sample_of(record.summary.id, &record.detail)?;
                if let Err(err) = visit(&sample) {
                    return Ok(Err(err));
                }
            }
        }
    }

    /// Deletes every sample, in short statements, and returns how many.
    pub fn clear(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        loop {
            let step = self
                .database()
                .write_unrecorded(|conn| samples::clear_step(conn, CLEAR_BATCH))?;
            if step == 0 {
                return Ok(total);
            }
            total += step as u64;
            std::thread::sleep(CLEAR_PAUSE);
        }
    }
}

fn filter_of(query: &SampleQuery) -> Result<SampleFilter, StoreError> {
    query.validate()?;
    Ok(SampleFilter {
        triggers: query.trigger_set()?,
        status_class: query.status_class,
        subsystem: query.subsystem.clone(),
        route: query.route.clone(),
        instance: query.instance.clone(),
        client: query.client.clone(),
        min_ms: query.min_ms,
        since_ms: query.since_ms,
        before_id: query.before_id,
        limit: query.limit,
    })
}

fn sample_of(id: i64, detail: &str) -> Result<Sample, StoreError> {
    let mut sample = Sample::from_detail_json(detail)
        .map_err(|_| StoreError::Internal(format!("sample {id} is unreadable")))?;
    sample.id = Some(id);
    Ok(sample)
}
