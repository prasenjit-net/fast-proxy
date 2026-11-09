use std::path::Path;
use std::sync::mpsc;
use std::thread::JoinHandle;

use tokio::sync::oneshot;

use crate::{Database, StoreError};

type Job = Box<dyn FnOnce(&mut Database) + Send>;

/// Async handle to the database. The connection lives on one dedicated
/// thread; jobs are sent over a channel, so async code never blocks on
/// SQLite and writes are naturally serialized. The data plane must never
/// use this: it reads compiled snapshots instead.
pub struct Store {
    jobs: Option<mpsc::Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

impl Store {
    /// Opens the database at `path` and starts its thread.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::start(Database::open(path)?)
    }

    /// An in-memory store, for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::start(Database::open_in_memory()?)
    }

    fn start(mut database: Database) -> Result<Self, StoreError> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let thread = std::thread::Builder::new()
            .name("fast-proxy-store".into())
            .spawn(move || {
                for job in inbox {
                    job(&mut database);
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            thread: Some(thread),
        })
    }

    /// Runs `job` on the store thread and returns its result.
    pub async fn run<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut Database) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let stopped = || StoreError::Internal("the store thread has stopped".into());
        let (reply, response) = oneshot::channel();
        let sender = self.jobs.as_ref().ok_or_else(stopped)?;
        sender
            .send(Box::new(move |database| {
                // The caller may have gone away; nothing to do then.
                let _ = reply.send(job(database));
            }))
            .map_err(|_| stopped())?;
        response.await.map_err(|_| stopped())?
    }

    /// Stops the thread and waits for the database file to close.
    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            // A panicked store thread has nothing left to clean up.
            let _ = thread.join();
        }
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::subsystems;
    use crate::test_support::subsystem_fields;
    use crate::Mutation;

    async fn create(store: &Store, name: &'static str) -> Result<u64, StoreError> {
        store
            .run(move |db| {
                db.write(
                    |conn| {
                        Ok(Mutation {
                            value: subsystems::create(conn, &subsystem_fields(name))?,
                            summary: format!("create {name}"),
                        })
                    },
                    |_| Ok(()),
                )
                .map(|written| written.version)
            })
            .await
    }

    #[tokio::test]
    async fn runs_jobs_in_order_on_one_thread() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(create(&store, "a").await.unwrap(), 1);
        assert_eq!(create(&store, "b").await.unwrap(), 2);
        let count = store
            .run(|db| Ok(db.load_all()?.subsystems.len()))
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn job_errors_reach_the_caller() {
        let store = Store::open_in_memory().unwrap();
        create(&store, "a").await.unwrap();
        assert!(matches!(
            create(&store, "a").await.unwrap_err(),
            StoreError::Conflict(_)
        ));
    }

    #[tokio::test]
    async fn concurrent_callers_are_serialized() {
        let store = std::sync::Arc::new(Store::open_in_memory().unwrap());
        let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let tasks: Vec<_> = names
            .into_iter()
            .map(|name| {
                let store = store.clone();
                tokio::spawn(async move { create(&store, name).await.unwrap() })
            })
            .collect();
        let mut versions = Vec::new();
        for task in tasks {
            versions.push(task.await.unwrap());
        }
        versions.sort_unstable();
        assert_eq!(versions, (1..=8).collect::<Vec<u64>>());
    }

    #[tokio::test]
    async fn data_survives_closing_and_reopening_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fast-proxy.db");
        let store = Store::open(&path).unwrap();
        create(&store, "shop").await.unwrap();
        store.close();

        let reopened = Store::open(&path).unwrap();
        let names = reopened
            .run(|db| {
                Ok(db
                    .load_all()?
                    .subsystems
                    .iter()
                    .map(|s| s.name.as_str().to_string())
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap();
        assert_eq!(names, ["shop"]);
    }

    #[tokio::test]
    async fn opening_a_file_in_a_missing_unwritable_place_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("file");
        std::fs::write(&blocker, b"x").unwrap();
        assert!(Store::open(&blocker.join("db")).is_err());
    }
}
