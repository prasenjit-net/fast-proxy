use std::path::Path;

use fast_proxy_model::Config;
use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::repository::load;
use crate::{revision, StoreError};

/// Bump together with `schema.sql`. Stored in `PRAGMA user_version`.
const SCHEMA_VERSION: i64 = 5;
/// The actor recorded for changes nobody asked for by name (the sweeper, tests).
/// The WAL is truncated to this size after a checkpoint.
const WAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;
const SYSTEM_ACTOR: &str = "system";
const SCHEMA_SQL: &str = include_str!("schema.sql");

/// The result of a successful write: the operation's value plus the
/// revision it created and the configuration as committed.
#[derive(Debug)]
pub struct Written<T> {
    pub value: T,
    pub version: u64,
    pub config: Config,
}

/// What a mutation did, with the human-readable revision summary
/// (for example `route created: /api in shop`).
#[derive(Debug)]
pub struct Mutation<T> {
    pub value: T,
    pub summary: String,
}

/// A synchronous handle on the SQLite file. It is not `Sync`; the async
/// [`Store`](crate::Store) keeps one on a dedicated thread.
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Opens (creating if needed) the database at `path`, enables WAL and
    /// foreign keys, and creates the schema in an empty file.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        Self::prepare(Connection::open(path)?)
    }

    /// Opens another connection to a database this process already opened, for
    /// the sample service: its writes go on a thread of their own, so a burst of
    /// sample batches never queues in front of a configuration write. Writers
    /// still take turns in SQLite; one that cannot get the lock within
    /// `busy_timeout` fails with an error instead of waiting.
    pub fn open_secondary(
        path: &Path,
        busy_timeout: std::time::Duration,
    ) -> Result<Self, StoreError> {
        let database = Self::prepare(Connection::open(path)?)?;
        database.conn.busy_timeout(busy_timeout)?;
        Ok(database)
    }

    /// Opens an existing database for reading only: nothing is created, no
    /// pragma that writes is set, and an empty file or one of another schema
    /// version is refused. For tools that inspect the stored configuration
    /// without touching it.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let found: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match found {
            SCHEMA_VERSION => Ok(Self { conn }),
            0 => Err(StoreError::NotFound(
                "a fast-proxy database (the file is empty)".into(),
            )),
            other => Err(StoreError::IncompatibleSchema {
                found: other,
                expected: SCHEMA_VERSION,
            }),
        }
    }

    /// An in-memory database, for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(conn: Connection) -> Result<Self, StoreError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        // Keeps the WAL file from staying large after a burst of writes (spike G2).
        conn.pragma_update(None, "journal_size_limit", WAL_SIZE_LIMIT_BYTES)?;
        // Returns the resulting mode as a row; in-memory databases stay "memory".
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        let database = Self { conn };
        database.ensure_schema()?;
        Ok(database)
    }

    fn ensure_schema(&self) -> Result<(), StoreError> {
        let found: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        match found {
            SCHEMA_VERSION => Ok(()),
            0 => {
                self.conn.execute_batch(SCHEMA_SQL)?;
                self.conn
                    .pragma_update(None, "user_version", SCHEMA_VERSION)?;
                Ok(())
            }
            other => Err(StoreError::IncompatibleSchema {
                found: other,
                expected: SCHEMA_VERSION,
            }),
        }
    }

    /// Runs a read-only closure against the connection.
    pub fn read<T>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        read(&self.conn)
    }

    /// The whole configuration as currently stored.
    pub fn load_all(&self) -> Result<Config, StoreError> {
        load::load_all(&self.conn)
    }

    /// The latest revision number (0 for a fresh database).
    pub fn current_version(&self) -> Result<u64, StoreError> {
        revision::current_version(&self.conn)
    }

    /// Runs `write` in one write transaction that records **no revision**: for
    /// high-volume data that is not configuration (samples). Nothing here may
    /// change `Config`; nothing is audited per call and the cluster generation
    /// does not move, so other instances do not reload.
    pub fn write_unrecorded<T>(
        &mut self,
        write: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = write(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// [`write_as`](Self::write_as) with the actor `system`.
    pub fn write<T>(
        &mut self,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError>,
        check: impl FnOnce(&Config) -> Result<(), String>,
    ) -> Result<Written<T>, StoreError> {
        self.write_as(SYSTEM_ACTOR, mutate, check)
    }

    /// Applies one change atomically, recording `actor` on its revision:
    ///
    /// 1. open a write transaction and run `mutate`,
    /// 2. load the would-be configuration and hand it to `check`,
    /// 3. only if both succeed, record a revision and commit.
    ///
    /// Any failure rolls everything back and leaves the database untouched.
    /// `check` is how callers veto a change (for example by compiling the
    /// routing snapshot) before it becomes durable.
    pub fn write_as<T>(
        &mut self,
        actor: &str,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError>,
        check: impl FnOnce(&Config) -> Result<(), String>,
    ) -> Result<Written<T>, StoreError> {
        let tx: Transaction<'_> = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Mutation { value, summary } = mutate(&tx)?;
        let config = load::load_all(&tx)?;
        check(&config).map_err(|message| StoreError::invalid_field("config", &message))?;
        let version = revision::append(&tx, actor, &summary, &config)?;
        tx.commit()?;
        Ok(Written {
            value,
            version,
            config,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::subsystems;
    use crate::test_support::{seed, subsystem_fields};

    #[test]
    fn creates_the_schema_in_an_empty_file_and_reopens_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("fast-proxy.db");
        {
            let mut db = Database::open(&path).unwrap();
            seed(&mut db, "shop", "s.example.com");
        }
        let db = Database::open(&path).unwrap();
        assert_eq!(db.load_all().unwrap().subsystems.len(), 1);
        assert_eq!(db.current_version().unwrap(), 1);
    }

    #[test]
    fn uses_wal_mode_and_enforces_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("x.db")).unwrap();
        let (mode, keys): (String, i64) = db
            .read(|c| {
                Ok((
                    c.pragma_query_value(None, "journal_mode", |r| r.get(0))?,
                    c.pragma_query_value(None, "foreign_keys", |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!(mode, "wal");
        assert_eq!(keys, 1);
    }

    #[test]
    fn a_read_only_open_reads_and_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        {
            let mut db = Database::open(&path).unwrap();
            seed(&mut db, "shop", "s.example.com");
        }
        let db = Database::open_read_only(&path).unwrap();
        assert_eq!(db.load_all().unwrap().subsystems.len(), 1);
        assert_eq!(db.current_version().unwrap(), 1);
        drop(db);
        // SQLite may leave its own -wal and -shm sidecars after reading a WAL
        // database; nothing else appears.
        let mut files: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !name.ends_with("-wal") && !name.ends_with("-shm"))
            .collect();
        files.sort();
        assert_eq!(files, ["x.db"]);
    }

    #[test]
    fn a_read_only_open_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        drop(Database::open(&path).unwrap());
        let db = Database::open_read_only(&path).unwrap();
        let err = db
            .read(|conn| {
                conn.execute(
                    "INSERT INTO vault (id, check_nonce, check_blob) VALUES (1, x'00', x'00')",
                    [],
                )
                .map_err(StoreError::from)
            })
            .unwrap_err();
        assert!(matches!(err, StoreError::Internal(_)), "{err}");
    }

    #[test]
    fn a_read_only_open_refuses_an_empty_file_a_missing_one_and_another_version() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.db");
        Connection::open(&empty).unwrap();
        assert!(matches!(
            Database::open_read_only(&empty).err().unwrap(),
            StoreError::NotFound(_)
        ));
        assert!(Database::open_read_only(&dir.path().join("missing.db")).is_err());
        assert!(
            !dir.path().join("missing.db").exists(),
            "nothing was created"
        );
        let old = dir.path().join("old.db");
        Connection::open(&old)
            .unwrap()
            .pragma_update(None, "user_version", 2)
            .unwrap();
        assert!(matches!(
            Database::open_read_only(&old).err().unwrap(),
            StoreError::IncompatibleSchema { found: 2, .. }
        ));
    }

    #[test]
    fn a_secondary_connection_shares_the_file_and_sets_the_wal_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let mut primary = Database::open(&path).unwrap();
        let mut secondary =
            Database::open_secondary(&path, std::time::Duration::from_secs(1)).unwrap();
        seed(&mut primary, "shop", "s.example.com");
        assert_eq!(
            secondary.load_all().unwrap().subsystems.len(),
            1,
            "it sees the primary's commit"
        );
        let (mode, limit): (String, i64) = secondary
            .read(|c| {
                Ok((
                    c.pragma_query_value(None, "journal_mode", |r| r.get(0))?,
                    c.pragma_query_value(None, "journal_size_limit", |r| r.get(0))?,
                ))
            })
            .unwrap();
        assert_eq!((mode.as_str(), limit), ("wal", WAL_SIZE_LIMIT_BYTES));
        secondary
            .write_unrecorded(|c| {
                c.execute(
                    "INSERT INTO vault (id, check_nonce, check_blob) VALUES (1, x'00', x'00')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let vault_rows: i64 = primary
            .read(|c| Ok(c.query_row("SELECT COUNT(*) FROM vault", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(vault_rows, 1, "the primary sees the secondary's commit");
    }

    #[test]
    fn writers_on_two_connections_take_turns_and_both_finish() {
        use crate::repository::samples::{self, NewSample};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        let mut primary = Database::open(&path).unwrap();
        let sample_path = path.clone();
        let writer = std::thread::spawn(move || {
            let mut secondary =
                Database::open_secondary(&sample_path, std::time::Duration::from_secs(5)).unwrap();
            for round in 0..40 {
                let batch: Vec<NewSample> = (0..50)
                    .map(|n| NewSample {
                        at_ms: round * 100 + n,
                        instance: "b".into(),
                        request_id: format!("r{round}-{n}"),
                        triggers: [fast_proxy_model::Trigger::Slow].into_iter().collect(),
                        subsystem: "shop".into(),
                        route: "/".into(),
                        client: "c".into(),
                        method: "GET".into(),
                        status: 200,
                        total_ms: 1,
                        detail: "x".repeat(1500),
                    })
                    .collect();
                secondary
                    .write_unrecorded(|c| samples::insert_batch(c, &batch))
                    .unwrap();
            }
        });
        for n in 0..40 {
            seed(&mut primary, &format!("s{n}"), &format!("s{n}.example.com"));
        }
        writer.join().unwrap();
        assert_eq!(
            primary.current_version().unwrap(),
            40,
            "only the configuration writes made revisions"
        );
        assert_eq!(primary.read(samples::usage).unwrap().rows, 2000);
    }

    #[test]
    fn a_secondary_that_cannot_get_the_write_lock_fails_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.db");
        drop(Database::open(&path).unwrap());
        let holder = Connection::open(&path).unwrap();
        holder.execute_batch("BEGIN IMMEDIATE").unwrap();
        let mut secondary =
            Database::open_secondary(&path, std::time::Duration::from_millis(100)).unwrap();
        let started = std::time::Instant::now();
        let err = secondary.write_unrecorded(|_| Ok(())).unwrap_err();
        assert!(matches!(err, StoreError::Internal(_)), "{err}");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "it gave up at the busy timeout"
        );
        holder.execute_batch("ROLLBACK").unwrap();
        secondary.write_unrecorded(|_| Ok(())).unwrap();
    }

    #[test]
    fn rejects_a_database_with_a_different_schema_version() {
        for found in [1, 2, 3, 4, 99] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("old.db");
            Connection::open(&path)
                .unwrap()
                .pragma_update(None, "user_version", found)
                .unwrap();
            let err = Database::open(&path).err().unwrap();
            assert!(matches!(
                err,
                StoreError::IncompatibleSchema { found: f, expected: SCHEMA_VERSION } if f == found
            ));
        }
    }

    #[test]
    fn a_failing_check_rolls_back_the_change_and_records_no_revision() {
        let mut db = Database::open_in_memory().unwrap();
        let err = db
            .write(
                |conn| {
                    Ok(Mutation {
                        value: subsystems::create(conn, &subsystem_fields("shop"))?,
                        summary: "create".into(),
                    })
                },
                |config| {
                    assert_eq!(config.subsystems.len(), 1, "check sees the would-be config");
                    Err("vetoed".into())
                },
            )
            .unwrap_err();
        assert!(matches!(&err, StoreError::Invalid(f) if f[0].message == "vetoed"));
        assert!(db.load_all().unwrap().subsystems.is_empty());
        assert_eq!(db.current_version().unwrap(), 0);
    }

    #[test]
    fn a_failing_mutation_rolls_back_its_earlier_statements() {
        let mut db = Database::open_in_memory().unwrap();
        let result: Result<Written<()>, _> = db.write(
            |conn| {
                subsystems::create(conn, &subsystem_fields("shop"))?;
                subsystems::create(conn, &subsystem_fields("shop"))?;
                Ok(Mutation {
                    value: (),
                    summary: "never".into(),
                })
            },
            |_| Ok(()),
        );
        assert!(matches!(result.unwrap_err(), StoreError::Conflict(_)));
        assert!(db.load_all().unwrap().subsystems.is_empty());
    }
}
