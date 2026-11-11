use std::cell::RefCell;
use std::sync::Arc;

use fast_proxy_auth::TokenCache;
use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_routing::{compile_all, MaintenanceInputs, Snapshot, SnapshotHolder};
use fast_proxy_store::repository::{audit, certificates, load, vault};
use fast_proxy_store::{revision, Connection, Database, Mutation, Store, StoreError};
use fast_proxy_telemetry::{AuditResult, Metrics, ReloadOutcome, SyncResult};
use fast_proxy_tls::certificate_readings;
use tokio::sync::{broadcast, Mutex};

use crate::change_context::committed_entry;
use crate::dev_certificates;
use crate::maintenance::stored_inputs;
use crate::metric_series::{live_endpoints, maintenance_readings};
use crate::tls_state::{Prepared, TlsSettings, TlsState, VaultStatus};
use crate::{Audited, ChangeContext, StartError};

/// The message of the `Conflict` a change returns when, on a second look inside
/// its transaction, there is nothing left to do (another instance got there
/// first). Callers translate it into success.
pub(crate) const NOTHING_TO_DO: &str = "nothing to do";

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
    pub(crate) store: Store,
    holder: Arc<SnapshotHolder>,
    /// Held across persist + swap so commit order always equals swap order.
    writer: Mutex<()>,
    changes: broadcast::Sender<ConfigChanged>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) tls: Arc<TlsState>,
    /// Names this process in audit entries, so a shared log says which instance wrote.
    pub(crate) instance: String,
    pub(crate) tokens: Arc<TokenCache>,
}

impl ConfigService {
    /// Loads the stored configuration, compiles it, and publishes it as the
    /// initial snapshot. Fails if the stored configuration does not compile.
    pub async fn start(store: Store) -> Result<Self, StartError> {
        Self::start_with_metrics(store, Arc::new(Metrics::new())).await
    }

    /// Like [`ConfigService::start`], reporting the config version and every
    /// accepted or rejected change to `metrics`. TLS is off.
    pub async fn start_with_metrics(
        store: Store,
        metrics: Arc<Metrics>,
    ) -> Result<Self, StartError> {
        Self::start_with_tls(store, metrics, TlsSettings::default()).await
    }

    /// Like [`ConfigService::start_with_metrics`], with the process TLS
    /// settings. Unlocks the vault when a key is needed, parses the stored
    /// certificates (an unreadable one is reported, never fatal), and fills any
    /// certificate gap while TLS is on.
    pub async fn start_with_tls(
        store: Store,
        metrics: Arc<Metrics>,
        settings: TlsSettings,
    ) -> Result<Self, StartError> {
        let tls_enabled = settings.enabled;
        let (config, version, has_certificates, check, maintenance) = store
            .run(|db| {
                db.read(|conn| {
                    Ok((
                        load::load_all(conn)?,
                        revision::current_version(conn)?,
                        !certificates::list(conn)?.is_empty(),
                        vault::get(conn)?,
                        stored_inputs(conn, now_ms())?,
                    ))
                })
            })
            .await?;
        let tls = Arc::new(TlsState::open(settings, has_certificates, check.as_ref())?);
        let prepared = store
            .run({
                let tls = tls.clone();
                move |db| db.read(|conn| tls.build(conn))
            })
            .await?;
        let snapshot = compile_all(
            version,
            &config,
            &RuntimeRegistry::new(),
            &tls.inputs(&prepared, false),
            &maintenance,
        )?;
        metrics.set_maintenance_active(&maintenance_readings(&snapshot));
        let holder = Arc::new(SnapshotHolder::new());
        holder.swap(snapshot);
        metrics.set_config_version(version);
        publish_certificates(&metrics, &prepared);
        tls.install(prepared.cache);
        let (changes, _) = broadcast::channel(64);
        let service = Self {
            store,
            holder,
            writer: Mutex::new(()),
            changes,
            metrics,
            tls,
            instance: new_instance_id(),
            tokens: Arc::new(TokenCache::new()),
        };
        service.refresh_tokens().await?;
        if tls_enabled {
            service.go_live_with_tls().await?;
        }
        Ok(service)
    }

    /// Switches the first snapshot from inert to TLS-on: a write that fills
    /// certificate gaps when there are any, else a recompile with no write.
    async fn go_live_with_tls(&self) -> Result<(), StartError> {
        let needs_certificates = self
            .store
            .run(|db| {
                {
                    db.read(|conn| {
                        let config = TlsState::config(conn)?;
                        let rows = certificates::list(conn)?;
                        Ok(!dev_certificates::plan(&config, &rows, now_ms()).is_empty())
                    })
                }
            })
            .await?;
        if needs_certificates {
            self.apply(
                &ChangeContext::system(),
                Audited::new("certificate.generate", "development certificates"),
                |_| {
                    Ok(Mutation {
                        value: (),
                        summary: "startup: development certificates generated".into(),
                    })
                },
            )
            .await?;
            return Ok(());
        }
        let previous = self.holder.load().runtime_registry();
        let prepared = self
            .store
            .run({
                let tls = self.tls.clone();
                move |db| db.read(|conn| tls.build(conn))
            })
            .await?;
        let (config, version, maintenance) = self
            .store
            .run(|db| {
                db.read(|conn| {
                    Ok((
                        load::load_all(conn)?,
                        revision::current_version(conn)?,
                        stored_inputs(conn, now_ms())?,
                    ))
                })
            })
            .await?;
        let inputs = self.tls.inputs(&prepared, true);
        let snapshot = compile_all(version, &config, &previous, &inputs, &maintenance)?;
        self.metrics
            .set_maintenance_active(&maintenance_readings(&snapshot));
        self.holder.swap(snapshot);
        self.tls.install(prepared.cache);
        Ok(())
    }

    /// Looks once for a change made by another instance (or process) sharing the
    /// database: when the stored revision is ahead of the live snapshot, the
    /// configuration and certificates are reloaded and swapped in, carrying the
    /// endpoint runtime state and the unchanged certificates over. Takes the
    /// writer lock, so it never interleaves with a local write; a local commit
    /// leaves the two versions equal, so it never triggers a reload.
    pub async fn sync_once(&self) -> SyncResult {
        let result = self.sync_under_lock().await;
        self.metrics.config_sync(result);
        result
    }

    async fn sync_under_lock(&self) -> SyncResult {
        let _writer = self.writer.lock().await;
        let live = self.holder.load().version;
        match self.store.run(|db| db.current_version()).await {
            Ok(stored) if stored <= live => return SyncResult::Unchanged,
            Ok(_) => {}
            Err(err) => {
                tracing::warn!("cluster sync cannot read the database version: {err}");
                return SyncResult::Failed;
            }
        }
        match self.reload_from_database().await {
            Ok(summary) => {
                let _ = self.changes.send(summary);
                SyncResult::Reloaded
            }
            Err(err) => {
                tracing::warn!("cluster sync could not apply a newer configuration: {err}");
                SyncResult::Failed
            }
        }
    }

    /// Loads the stored state, compiles it against the live runtime state and
    /// swaps it in. The caller holds the writer lock.
    async fn reload_from_database(&self) -> Result<ConfigChanged, StartError> {
        let previous = self.holder.load().runtime_registry();
        let tls = self.tls.clone();
        let (config, version, prepared, maintenance) = self
            .store
            .run(move |db| {
                db.read(|conn| {
                    tls.refresh_vault(conn)?;
                    Ok((
                        load::load_all(conn)?,
                        revision::current_version(conn)?,
                        tls.build(conn)?,
                        stored_inputs(conn, now_ms())?,
                    ))
                })
            })
            .await?;
        let inputs = self.tls.inputs(&prepared, self.tls.settings.enabled);
        let snapshot = compile_all(version, &config, &previous, &inputs, &maintenance)?;
        self.metrics
            .set_maintenance_active(&maintenance_readings(&snapshot));
        self.metrics
            .retain_upstream_series(&live_endpoints(&snapshot));
        self.metrics.set_config_version(version);
        publish_certificates(&self.metrics, &prepared);
        self.tls.install(prepared.cache);
        self.holder.swap(snapshot);
        self.refresh_tokens().await?;
        Ok(ConfigChanged {
            version,
            summary: "configuration synchronized from the database".into(),
        })
    }

    /// Re-reads the database now and recompiles, whether or not its version is
    /// ahead of the live one: for a database edited by hand, an instance that
    /// does not poll, or a restore. Returns the version now live. A stored
    /// configuration that does not compile leaves the old snapshot in place.
    pub async fn reload(&self) -> Result<u64, StartError> {
        let _writer = self.writer.lock().await;
        let result = self.reload_from_database().await;
        match &result {
            Ok(changed) => {
                self.metrics.config_reload(ReloadOutcome::Applied);
                let _ = self.changes.send(changed.clone());
            }
            Err(_) => self.metrics.config_reload(ReloadOutcome::Rejected),
        }
        result.map(|changed| changed.version)
    }

    /// Polls for changes made by other instances every `interval` until
    /// `shutdown` is cancelled. A zero interval turns the sync off: the future
    /// resolves at once.
    pub async fn run_sync(
        self: Arc<Self>,
        interval: std::time::Duration,
        shutdown: tokio_util::sync::CancellationToken,
    ) {
        if interval.is_zero() {
            return;
        }
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(interval) => {}
            }
            self.sync_once().await;
        }
    }

    /// Whether stored private keys can be read.
    pub fn vault_status(&self) -> VaultStatus {
        self.tls.status()
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
    ///
    /// `context` and `audited` say who made the change and what it was: the
    /// revision names the actor, and the audit entry is written in the same
    /// transaction, so a committed change always has exactly one entry and a
    /// vetoed one has none.
    pub async fn apply<T: Send + 'static>(
        &self,
        context: &ChangeContext,
        audited: Audited,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError> + Send + 'static,
    ) -> Result<Applied<T>, StoreError> {
        let result = self.apply_under_lock(context, audited, mutate).await;
        match &result {
            Ok(applied) => {
                self.metrics.set_config_version(applied.version);
                self.metrics.config_reload(ReloadOutcome::Applied);
                // The entry went in with the change, in the same transaction.
                self.metrics.audit_entry(AuditResult::Ok);
            }
            Err(_) => self.metrics.config_reload(ReloadOutcome::Rejected),
        }
        result
    }

    async fn apply_under_lock<T: Send + 'static>(
        &self,
        context: &ChangeContext,
        audited: Audited,
        mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError> + Send + 'static,
    ) -> Result<Applied<T>, StoreError> {
        let _writer = self.writer.lock().await;
        // Endpoint runtime state (health, ejection, latency) is carried from
        // the live snapshot into the new one; the writer lock keeps the live
        // snapshot from changing under us.
        let previous = self.holder.load().runtime_registry();
        let tls = self.tls.clone();
        let context = context.clone();
        let instance = self.instance.clone();
        let (written, compiled, summary, prepared) = self
            .store
            .run(move |db| {
                let actor = context.actor.clone();
                let audited_mutate = move |conn: &Connection| {
                    let mutation = mutate(conn)?;
                    let version = revision::current_version(conn)? + 1;
                    let entry = committed_entry(
                        &context,
                        &audited,
                        &instance,
                        &mutation.summary,
                        version,
                        now_ms(),
                    );
                    audit::append(conn, &entry)?;
                    Ok(mutation)
                };
                persist_and_compile(db, &actor, audited_mutate, &previous, &tls)
            })
            .await?;
        let mut snapshot = compiled.ok_or_else(|| {
            StoreError::Internal("a committed change produced no snapshot".into())
        })?;
        snapshot.version = written.version;
        self.metrics
            .retain_upstream_series(&live_endpoints(&snapshot));
        publish_certificates(&self.metrics, &prepared);
        self.metrics
            .set_maintenance_active(&maintenance_readings(&snapshot));
        self.tls.install(prepared.cache);
        self.holder.swap(snapshot);
        self.refresh_tokens().await?;
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
    pub async fn rollback(
        &self,
        context: &ChangeContext,
        version: u64,
    ) -> Result<Applied<()>, StoreError> {
        self.apply(
            context,
            Audited::new("revision.rollback", format!("v{version}")),
            move |conn| {
                fast_proxy_store::revision::restore(conn, version)?;
                Ok(Mutation {
                    value: (),
                    summary: format!("rollback to v{version}"),
                })
            },
        )
        .await
    }
}

type PersistOutcome<T> = (
    fast_proxy_store::Written<T>,
    Option<Snapshot>,
    String,
    Prepared,
);

fn publish_certificates(metrics: &Metrics, prepared: &Prepared) {
    metrics.cert_cache_rebuilt(
        prepared.stats.hits,
        prepared.stats.misses,
        prepared.stats.unreadable,
    );
    metrics.set_certificate_readings(&certificate_readings(&prepared.cache, now_ms()));
}

/// A name for this process in audit entries: its pid and start time.
fn new_instance_id() -> String {
    format!("{}-{:x}", std::process::id(), now_ms())
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// Runs on the store thread: mutate, fill certificate gaps, compile as the
/// veto (with the would-be certificates), commit.
fn persist_and_compile<T>(
    db: &mut Database,
    actor: &str,
    mutate: impl FnOnce(&Connection) -> Result<Mutation<T>, StoreError>,
    previous: &RuntimeRegistry,
    tls: &TlsState,
) -> Result<PersistOutcome<T>, StoreError> {
    let mut compiled = None;
    let mut summary = String::new();
    let prepared: RefCell<Option<Prepared>> = RefCell::new(None);
    let maintenance: RefCell<Option<MaintenanceInputs>> = RefCell::new(None);
    let written = db.write_as(
        actor,
        |conn| {
            let mutation = mutate(conn)?;
            summary = mutation.summary.clone();
            *prepared.borrow_mut() = Some(tls.prepare(conn, now_ms())?);
            // Read after the mutation, so a scope cascaded away with its
            // subsystem or route is not compiled.
            *maintenance.borrow_mut() = Some(stored_inputs(conn, now_ms())?);
            Ok(mutation)
        },
        |config| {
            let borrowed = prepared.borrow();
            let Some(ready) = borrowed.as_ref() else {
                return Err("certificates were not prepared".to_string());
            };
            let borrowed_maintenance = maintenance.borrow();
            let Some(scopes) = borrowed_maintenance.as_ref() else {
                return Err("maintenance was not read".to_string());
            };
            let inputs = tls.inputs(ready, tls.settings.enabled);
            match compile_all(0, config, previous, &inputs, scopes) {
                Ok(snapshot) => {
                    compiled = Some(snapshot);
                    Ok(())
                }
                Err(err) => Err(err.to_string()),
            }
        },
    )?;
    let prepared = prepared.into_inner().ok_or_else(|| {
        StoreError::Internal("a committed change prepared no certificates".into())
    })?;
    Ok((written, compiled, summary, prepared))
}
