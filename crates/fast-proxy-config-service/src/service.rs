use std::sync::Arc;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_routing::{compile, Snapshot, SnapshotHolder};
use fast_proxy_store::{Connection, Database, Mutation, Store, StoreError};
use fast_proxy_telemetry::{Metrics, ReloadOutcome};
use tokio::sync::{broadcast, Mutex};

use crate::StartError;

/// Broadcast after every applied change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigChanged {
    pub version: u64,
    pub summary: String,
}

/// The outcome of an applied change.
#[derive(Debug)]
pub struct Applied<T> {
    pub value: T,
    /// The revision this change created, now live in the data plane.
    pub version: u64,
}

/// Owns the store and the live snapshot, and is the only writer of both.
pub struct ConfigService {
    store: Store,
    holder: Arc<SnapshotHolder>,
    /// Held across persist + swap so commit order always equals swap order.
    writer: Mutex<()>,
    changes: broadcast::Sender<ConfigChanged>,
    metrics: Arc<Metrics>,
}

impl ConfigService {
    /// Loads the stored configuration, compiles it, and publishes it as the
    /// initial snapshot. Fails if the stored configuration does not compile.
    pub async fn start(store: Store) -> Result<Self, StartError> {
        Self::start_with_metrics(store, Arc::new(Metrics::new())).await
    }

    /// Like [`ConfigService::start`], reporting the config version and every
    /// accepted or rejected change to `metrics`.
    pub async fn start_with_metrics(
        store: Store,
        metrics: Arc<Metrics>,
    ) -> Result<Self, StartError> {
        let (config, version) = store
            .run(|db| Ok((db.load_all()?, db.current_version()?)))
            .await?;
        let holder = Arc::new(SnapshotHolder::new());
        holder.swap(compile(version, &config, &RuntimeRegistry::new())?);
        metrics.set_config_version(version);
        let (changes, _) = broadcast::channel(64);
        Ok(Self {
            store,
            holder,
            writer: Mutex::new(()),
            changes,
            metrics,
        })
    }

    /// The live snapshot holder, shared with the data plane.
    pub fn snapshot_holder(&self) -> Arc<SnapshotHolder> {
        self.holder.clone()
    }

    /// Receives one event per applied change.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigChanged> {
        self.changes.subscribe()
    }

    /// Runs a read-only job against the database.
    pub async fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<Applied<T>, StoreError> {
        let (value, version) = self
            .store
            .run(move |db| Ok((db.read(read)?, db.current_version()?)))
            .await?;
        Ok(Applied { value, version })
    }

    /// Applies one change:
    ///
    /// 1. take the writer lock,
    /// 2. run `mutate` in a transaction without committing,
    /// 3. compile the would-be configuration, which vetoes the change on any
    ///    violated rule (the transaction rolls back, the old snapshot stays),
    /// 4. commit and record the revision,
    /// 5. swap the new snapshot in and broadcast [`ConfigChanged`].
    pub async fn apply<T: Send + 'static>(
        &self,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError> + Send + 'static,
    ) -> Result<Applied<T>, StoreError> {
        let result = self.apply_under_lock(mutate).await;
        match &result {
            Ok(applied) => {
                self.metrics.set_config_version(applied.version);
                self.metrics.config_reload(ReloadOutcome::Applied);
            }
            Err(_) => self.metrics.config_reload(ReloadOutcome::Rejected),
        }
        result
    }

    async fn apply_under_lock<T: Send + 'static>(
        &self,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError> + Send + 'static,
    ) -> Result<Applied<T>, StoreError> {
        let _writer = self.writer.lock().await;
        // Endpoint runtime state (health, ejection, latency) is carried from
        // the live snapshot into the new one; the writer lock keeps the live
        // snapshot from changing under us.
        let previous = self.holder.load().runtime_registry();
        let (written, compiled, summary) = self
            .store
            .run(move |db| persist_and_compile(db, mutate, &previous))
            .await?;
        let mut snapshot = compiled.ok_or_else(|| {
            StoreError::Internal("a committed change produced no snapshot".into())
        })?;
        snapshot.version = written.version;
        self.holder.swap(snapshot);
        // send() only fails when nobody is subscribed, which is fine.
        let _ = self.changes.send(ConfigChanged {
            version: written.version,
            summary,
        });
        Ok(Applied {
            value: written.value,
            version: written.version,
        })
    }

    /// Re-applies the configuration stored in `version` as a new revision.
    pub async fn rollback(&self, version: u64) -> Result<Applied<()>, StoreError> {
        self.apply(move |conn| {
            fast_proxy_store::revision::restore(conn, version)?;
            Ok(Mutation {
                value: (),
                summary: format!("rollback to v{version}"),
            })
        })
        .await
    }
}

type PersistOutcome<T> = (fast_proxy_store::Written<T>, Option<Snapshot>, String);

/// Runs on the store thread: mutate, compile as the veto, commit.
fn persist_and_compile<T>(
    db: &mut Database,
    mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError>,
    previous: &RuntimeRegistry,
) -> Result<PersistOutcome<T>, StoreError> {
    let mut compiled = None;
    let mut summary = String::new();
    let written = db.write(
        |conn| {
            let mutation = mutate(conn)?;
            summary = mutation.summary.clone();
            Ok(mutation)
        },
        |config| match compile(0, config, previous) {
            Ok(snapshot) => {
                compiled = Some(snapshot);
                Ok(())
            }
            Err(err) => Err(err.to_string()),
        },
    )?;
    Ok((written, compiled, summary))
}
