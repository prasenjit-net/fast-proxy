//! The batched writer: the sampler's sink over the database, on its own
//! connection, one transaction per batch and no revision (ADR 0022). A batch
//! that cannot be written (database busy or full) is reported to the sampler,
//! which drops it, counts it and backs off.

use std::path::Path;
use std::time::Duration;

use fast_proxy_sampling::{Sample, SampleSink, SinkError};
use fast_proxy_store::repository::samples;
use fast_proxy_store::{Database, StoreError};

use crate::convert::to_row;

/// How long a batch waits for the database's write lock before giving up.
pub const WRITE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// Writes batches of samples to the shared database.
pub struct SampleWriter {
    database: Database,
}

impl SampleWriter {
    /// Opens the writer's own connection to the database at `path`.
    pub fn open(path: &Path, busy_timeout: Duration) -> Result<Self, StoreError> {
        Ok(Self::from_database(Database::open_secondary(
            path,
            busy_timeout,
        )?))
    }

    /// A writer over a connection somebody else opened.
    pub fn from_database(database: Database) -> Self {
        Self { database }
    }
}

impl SampleSink for SampleWriter {
    fn write_batch(&mut self, batch: Vec<Sample>) -> Result<(), SinkError> {
        let rows: Vec<_> = batch.iter().map(to_row).collect();
        self.database
            .write_unrecorded(|conn| samples::insert_batch(conn, &rows))
            .map(|_| ())
            .map_err(|err| SinkError(err.to_string()))
    }
}
