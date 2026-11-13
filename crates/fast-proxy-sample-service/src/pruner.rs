//! Retention (decision J18): the oldest samples go first, by age, then by rows,
//! then by bytes, as set in the shared settings row. Every instance runs a
//! pruner; the work is idempotent and needs no leader. Deletes are short
//! statements of a bounded number of rows, so the write lock is never held long.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use fast_proxy_store::repository::samples::{self, Usage};
use fast_proxy_store::repository::sampling;
use fast_proxy_store::{Database, StoreError};

const DAY_MS: i64 = 86_400_000;

/// What one pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    pub deleted: u64,
    /// What the table holds afterwards.
    pub usage: Usage,
}

/// Prunes the table to the retention in the stored settings, `batch` rows per
/// statement, pausing `pause` between statements so other writers get in (SQLite
/// hands the write lock to whoever asks first, and a waiting writer sleeps
/// between attempts, so a pruner that asks again at once would starve it).
///
/// The samples that are too old go first, through the time index. The excess over
/// the row and byte limits is then removed oldest first, the table being
/// measured (from narrow indexes, outside any write transaction) before each
/// step, so no statement scans the table while it holds the lock.
pub fn prune_once(
    database: &mut Database,
    now_ms: i64,
    batch: u32,
    pause: Duration,
) -> Result<PruneReport, StoreError> {
    let settings = database.read(sampling::get_settings)?;
    let cutoff = now_ms.saturating_sub(i64::from(settings.retention_days) * DAY_MS);
    let mut deleted = 0u64;
    loop {
        let step =
            database.write_unrecorded(|conn| samples::delete_older_than(conn, cutoff, batch))?;
        if step == 0 {
            break;
        }
        deleted += step as u64;
        std::thread::sleep(pause);
    }
    // The table is measured again before every step, outside the write lock
    // (an index-only read), so two pruners racing each over-delete by at most
    // the one step they were in.
    loop {
        let excess = database
            .read(samples::usage)?
            .excess(settings.max_rows, settings.max_bytes);
        if excess.is_none() {
            break;
        }
        let gone = database
            .write_unrecorded(|conn| samples::delete_oldest_covering(conn, excess, batch))?;
        if gone.rows == 0 {
            break;
        }
        deleted += gone.rows;
        std::thread::sleep(pause);
    }
    Ok(PruneReport {
        deleted,
        usage: database.read(samples::usage)?,
    })
}

/// How the pruner thread runs.
#[derive(Debug, Clone, Copy)]
pub struct PrunerConfig {
    /// Time between passes.
    pub interval: Duration,
    /// Rows deleted per statement.
    pub batch: u32,
    /// Pause between statements.
    pub pause: Duration,
    pub busy_timeout: Duration,
}

impl Default for PrunerConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            batch: 500,
            pause: Duration::from_millis(25),
            busy_timeout: Duration::from_secs(2),
        }
    }
}

/// Counters of the pruner thread, for the metrics.
#[derive(Debug, Default)]
pub struct PrunerStats {
    deleted: AtomicU64,
    failed_passes: AtomicU64,
    rows: AtomicU64,
    bytes: AtomicU64,
}

/// A reading of [`PrunerStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrunerReading {
    pub deleted: u64,
    pub failed_passes: u64,
    /// The table's rows and bytes after the last pass.
    pub rows: u64,
    pub bytes: u64,
}

impl PrunerStats {
    pub fn read(&self) -> PrunerReading {
        PrunerReading {
            deleted: self.deleted.load(Ordering::Relaxed),
            failed_passes: self.failed_passes.load(Ordering::Relaxed),
            rows: self.rows.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
        }
    }
}

/// The running pruner thread.
pub struct Pruner {
    stop: Sender<()>,
    thread: JoinHandle<()>,
    stats: Arc<PrunerStats>,
}

impl Pruner {
    /// Starts a thread that prunes at once and then every `config.interval`.
    /// A pass that fails (the database is busy) is counted and tried again at
    /// the next one.
    pub fn start(path: &Path, config: PrunerConfig) -> Result<Self, StoreError> {
        let mut database = Database::open_secondary(path, config.busy_timeout)?;
        let (stop, stopped) = mpsc::channel::<()>();
        let stats = Arc::new(PrunerStats::default());
        let thread_stats = stats.clone();
        let thread = std::thread::Builder::new()
            .name("fast-proxy-sample-pruner".into())
            .spawn(move || loop {
                match prune_once(&mut database, now_ms(), config.batch, config.pause) {
                    Ok(report) => {
                        // The usage first: whoever sees the deletions has the table's size.
                        thread_stats
                            .rows
                            .store(report.usage.rows, Ordering::Relaxed);
                        thread_stats
                            .bytes
                            .store(report.usage.bytes, Ordering::Relaxed);
                        thread_stats
                            .deleted
                            .fetch_add(report.deleted, Ordering::Relaxed);
                    }
                    Err(_) => {
                        thread_stats.failed_passes.fetch_add(1, Ordering::Relaxed);
                    }
                }
                match stopped.recv_timeout(config.interval) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                }
            })?;
        Ok(Self {
            stop,
            thread,
            stats,
        })
    }

    pub fn stats(&self) -> &Arc<PrunerStats> {
        &self.stats
    }

    /// Stops the thread after the pass it is in.
    pub fn stop(self) {
        drop(self.stop);
        let _ = self.thread.join();
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
