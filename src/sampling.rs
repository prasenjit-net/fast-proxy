//! Request sampling for the running process: the sampler's thread with the
//! database writer as its sink, the retention pruner, and a periodic task that
//! keeps the redactor, the routes' latency windows and the metrics current.
//! The data plane only sees the [`Sampler`] handle (ADR 0022).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_balancing::monotonic_ms;
use fast_proxy_config_service::ConfigChanged;
use fast_proxy_routing::{Snapshot, SnapshotHolder};
use fast_proxy_sample_service::{
    Pruner, PrunerConfig, PrunerStats, SampleWriter, WRITE_LOCK_TIMEOUT,
};
use fast_proxy_sampling::{Redactor, Sampler, SamplerConfig, SamplerRuntime};
use fast_proxy_store::StoreError;
use fast_proxy_telemetry::{Metrics, SamplerReading};
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::config::SamplingConfig;

/// How often the metrics are refreshed from the sampler and the pruner.
const METRICS_INTERVAL: Duration = Duration::from_secs(1);
/// Every this many metric refreshes, the routes' p99 are recomputed.
const P99_EVERY: u32 = 5;

/// Why sampling could not be started.
#[derive(Debug, Error)]
pub enum SamplingError {
    #[error("cannot open the database for request sampling: {0}")]
    Store(#[from] StoreError),
    #[error("cannot start the sampler thread: {0}")]
    Thread(#[from] std::io::Error),
}

/// The parts of request sampling that run beside the data plane.
pub struct Sampling {
    runtime: SamplerRuntime,
    pruner: Pruner,
}

impl Sampling {
    /// Starts the sampler thread (writing to the database at `database`) and the
    /// pruner. `snapshot` provides the first redaction additions.
    pub fn start(
        config: &SamplingConfig,
        database: &Path,
        instance: &str,
        snapshot: &Snapshot,
    ) -> Result<Self, SamplingError> {
        let writer = SampleWriter::open(database, WRITE_LOCK_TIMEOUT)?;
        let runtime = SamplerRuntime::start(
            SamplerConfig {
                queue_depth: config.queue_depth,
                batch_size: config.batch_size,
                batch_interval: config.batch_interval,
                body_budget_bytes: config.body_budget_bytes,
                instance: instance.to_string(),
            },
            redactor_for(snapshot),
            writer,
        )?;
        let pruner = Pruner::start(database, PrunerConfig::default())?;
        Ok(Self { runtime, pruner })
    }

    /// The handle the data plane reports kept requests to.
    pub fn sampler(&self) -> Arc<Sampler> {
        Arc::new(self.runtime.handle())
    }

    /// Starts the periodic task: it rebuilds the redactor whenever the
    /// configuration changes (the additions to the redaction lists and the
    /// maintenance bypass headers are in the snapshot), recomputes the routes'
    /// p99, and publishes the sampler's and the pruner's counts as metrics. It
    /// stops with `stop`.
    pub fn spawn_upkeep(
        &self,
        holder: Arc<SnapshotHolder>,
        changes: broadcast::Receiver<ConfigChanged>,
        metrics: Arc<Metrics>,
        stop: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(upkeep(
            self.runtime.handle(),
            self.pruner.stats().clone(),
            holder,
            changes,
            metrics,
            stop,
        ))
    }

    /// Writes what is still queued and stops the threads. Blocks until done.
    pub fn shutdown(self) {
        self.runtime.shutdown();
        self.pruner.stop();
    }
}

async fn upkeep(
    sampler: Sampler,
    pruner: Arc<PrunerStats>,
    holder: Arc<SnapshotHolder>,
    mut changes: broadcast::Receiver<ConfigChanged>,
    metrics: Arc<Metrics>,
    stop: CancellationToken,
) {
    let mut ticker = tokio::time::interval(METRICS_INTERVAL);
    let mut ticks = 0u32;
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = ticker.tick() => {
                ticks = ticks.wrapping_add(1);
                if ticks.is_multiple_of(P99_EVERY) {
                    publish_p99s(&holder.load());
                }
                metrics.publish_sampler(reading(&sampler, &pruner));
            }
            change = changes.recv() => match change {
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    sampler.set_redactor(redactor_for(&holder.load()));
                }
                Err(RecvError::Closed) => return,
            }
        }
    }
}

fn publish_p99s(snapshot: &Snapshot) {
    let now = monotonic_ms();
    for window in snapshot.sampling.windows() {
        window.publish(now);
    }
}

fn reading(sampler: &Sampler, pruner: &PrunerStats) -> SamplerReading {
    let sampled = sampler.stats().read();
    let pruned = pruner.read();
    SamplerReading {
        queue_full: sampled.queue_full,
        store_error: sampled.store_error,
        written: sampled.written,
        queued: sampled.queued,
        body_budget_used: sampler.budget().used(),
        pruned: pruned.deleted,
        table_rows: pruned.rows,
        table_bytes: pruned.bytes,
    }
}

/// The redactor for the settings and the maintenance scopes of `snapshot`: the
/// operator's additions, and the bypass header of every scope (a secret).
pub(crate) fn redactor_for(snapshot: &Snapshot) -> Redactor {
    let scopes = snapshot
        .maintenance
        .iter()
        .chain(snapshot.subsystems.values().flat_map(|subsystem| {
            subsystem.maintenance.iter().chain(
                subsystem
                    .routes
                    .iter()
                    .flat_map(|route| route.maintenance.iter()),
            )
        }));
    let bypass_headers: Vec<String> = scopes
        .filter_map(|scope| scope.bypass_header_name().map(str::to_string))
        .collect();
    Redactor::new(&snapshot.sampling_settings, &bypass_headers)
}

#[cfg(test)]
mod tests {
    use fast_proxy_balancing::RuntimeRegistry;
    use fast_proxy_model::{
        BypassHeader, BypassRules, MaintenanceSettings, MaintenanceTarget, SamplingSettings,
        SecretHash, SubsystemId,
    };
    use fast_proxy_routing::{
        compile_full, MaintenanceInputs, SamplingInputs, SamplingRuntime, TlsInputs,
    };
    use fast_proxy_telemetry::Metrics;

    use super::*;

    fn maintenance(header: Option<&str>) -> MaintenanceSettings {
        MaintenanceSettings {
            message: String::new(),
            body: None,
            retry_after_secs: None,
            bypass: BypassRules {
                cidrs: Vec::new(),
                header: header.map(|name| BypassHeader {
                    name: name.into(),
                    secret_hash: SecretHash::new([1; 32]),
                }),
            },
            expires_at_ms: None,
            set_by: "ops".into(),
            set_at_ms: 0,
        }
    }

    fn snapshot_with(
        scopes: Vec<(MaintenanceTarget, MaintenanceSettings)>,
        settings: SamplingSettings,
    ) -> Snapshot {
        let config = fast_proxy_model::Config {
            subsystems: vec![fast_proxy_dataplane_test_site()],
        };
        compile_full(
            1,
            &config,
            &RuntimeRegistry::new(),
            &TlsInputs::off(),
            &MaintenanceInputs { scopes, now_ms: 0 },
            &SamplingInputs {
                settings,
                overrides: Vec::new(),
                runtime: SamplingRuntime::new(),
            },
        )
        .unwrap()
    }

    /// One subsystem with a route, built by hand: this crate has no test
    /// builders of its own.
    fn fast_proxy_dataplane_test_site() -> fast_proxy_model::SubsystemConfig {
        use fast_proxy_model::*;
        let addr = EndpointAddr::parse("127.0.0.1:3000").unwrap();
        SubsystemConfig {
            id: SubsystemId(1),
            name: SubsystemName::parse("shop").unwrap(),
            enabled: true,
            description: String::new(),
            https: HttpsPolicy::default(),
            hosts: vec![HostPattern::parse("shop.example.com").unwrap()],
            upstream_groups: vec![UpstreamGroupConfig {
                id: UpstreamGroupId(1),
                name: "backend".into(),
                policy: GroupPolicy::default(),
                endpoints: vec![EndpointConfig {
                    id: EndpointId(1),
                    addr,
                    weight: 1,
                    state: EndpointState::Enabled,
                }],
            }],
            routes: vec![RouteConfig {
                id: RouteId(7),
                prefix: RoutePrefix::parse("/").unwrap(),
                upstream_group_id: UpstreamGroupId(1),
                strip_prefix: false,
                upstream_prefix: UpstreamPrefix::default(),
                preserve_host: true,
                connect_timeout: Duration::from_secs(5),
                response_timeout: Duration::from_secs(30),
                retry: RetryPolicy::default(),
                enabled: true,
            }],
        }
    }

    #[test]
    fn the_additions_and_every_scopes_bypass_header_are_redacted() {
        let settings = SamplingSettings {
            redact_headers: vec!["x-internal".into()],
            ..SamplingSettings::default()
        };
        let snapshot = snapshot_with(
            vec![
                (
                    MaintenanceTarget::Global,
                    maintenance(Some("X-Global-Pass")),
                ),
                (
                    MaintenanceTarget::Subsystem(SubsystemId(1)),
                    maintenance(Some("X-Shop-Pass")),
                ),
                (
                    MaintenanceTarget::Route(fast_proxy_model::RouteId(7)),
                    maintenance(Some("X-Route-Pass")),
                ),
            ],
            settings,
        );
        let redactor = redactor_for(&snapshot);
        for name in [
            "x-internal",
            "x-global-pass",
            "x-shop-pass",
            "x-route-pass",
            "authorization",
        ] {
            assert!(redactor.is_sensitive_header(name), "{name}");
        }
        assert!(!redactor.is_sensitive_header("x-request-id"));
    }

    #[test]
    fn with_no_scopes_only_the_settings_apply() {
        let redactor = redactor_for(&snapshot_with(Vec::new(), SamplingSettings::default()));
        assert!(redactor.is_sensitive_header("cookie"));
        assert!(!redactor.is_sensitive_header("x-global-pass"));
    }

    fn raw_sample(query: &str) -> fast_proxy_sampling::RawSample {
        use fast_proxy_model::Trigger;
        use fast_proxy_sampling::{
            Answer, AttemptLog, Outcome, RawSample, RequestHead, UnusualFlags,
        };
        RawSample {
            at_ms: 1,
            request_id: "req".into(),
            triggers: [Trigger::Slow].into_iter().collect(),
            subsystem: "shop".into(),
            route: "/".into(),
            head: RequestHead {
                method: "GET".into(),
                path: "/".into(),
                query: Some(query.into()),
                headers: Vec::new(),
                flags: UnusualFlags::default(),
                client: "203.0.113.9:1".parse().unwrap(),
            },
            outcome: Outcome {
                status: 200,
                elapsed_ms: 5000,
                answer: Answer::Upstream,
                attempts: AttemptLog::default(),
                bytes_in: 0,
                bytes_out: 0,
                client_cancelled: false,
            },
            attempts: Vec::new(),
            response_headers: Vec::new(),
            request_body: None,
            response_body: None,
            body_omitted: None,
            suppressed: None,
            reservation: None,
        }
    }

    async fn stored_query(
        reader: &fast_proxy_sample_service::SampleReader,
        count: usize,
    ) -> Vec<String> {
        let started = std::time::Instant::now();
        loop {
            let listed = reader
                .list(&fast_proxy_sampling::SampleQuery::default())
                .unwrap();
            if listed.len() >= count {
                let mut queries = Vec::new();
                for summary in listed {
                    let sample = reader.get(summary.id).unwrap();
                    queries.push(sample.request.query.unwrap_or_default());
                }
                return queries;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the sample was never written"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn a_configuration_change_gives_later_samples_the_new_redaction_and_the_metrics_follow() {
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("fp.db");
        let _ = fast_proxy_store::Database::open(&database).unwrap();
        let holder = Arc::new(SnapshotHolder::new());
        let config = SamplingConfig {
            batch_interval: Duration::from_millis(10),
            batch_size: 1,
            ..SamplingConfig::default()
        };
        let sampling = Sampling::start(&config, &database, "test", &holder.load()).unwrap();
        let sampler = sampling.sampler();
        let (changes, receiver) = broadcast::channel(8);
        let metrics = Arc::new(Metrics::new());
        let stop = CancellationToken::new();
        let upkeep = sampling.spawn_upkeep(holder.clone(), receiver, metrics.clone(), stop.clone());
        let reader =
            fast_proxy_sample_service::SampleReader::open(&database, Duration::from_secs(5))
                .unwrap();

        sampler.try_submit(raw_sample("page=3&token=ZQ")).unwrap();
        assert_eq!(stored_query(&reader, 1).await, ["page=3&token=[redacted]"]);

        // The operator adds `page` to the redaction list; the change is announced.
        let settings = SamplingSettings {
            redact_query_params: vec!["page".into()],
            ..SamplingSettings::default()
        };
        holder.swap(snapshot_with(Vec::new(), settings));
        changes
            .send(ConfigChanged {
                version: 2,
                summary: "sampling settings updated".into(),
            })
            .unwrap();
        // The refresh is asynchronous: submit until a sample shows it.
        let started = std::time::Instant::now();
        loop {
            sampler.try_submit(raw_sample("page=3")).unwrap();
            let queries = stored_query(&reader, 2).await;
            if queries.iter().any(|q| q == "page=[redacted]") {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "the new redaction never applied: {queries:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // The metrics are refreshed on their own tick.
        let started = std::time::Instant::now();
        while !metrics
            .render()
            .contains("fastproxy_samples_written_total 1")
            && !metrics
                .render()
                .contains("fastproxy_samples_written_total 2")
        {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "{}",
                metrics.render()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        stop.cancel();
        upkeep.await.unwrap();
        sampling.shutdown();
    }

    #[test]
    fn the_readings_of_the_sampler_and_the_pruner_become_one_metrics_reading() {
        use std::time::Duration;
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("fp.db");
        let _ = fast_proxy_store::Database::open(&database).unwrap();
        let snapshot = Snapshot::empty();
        let config = SamplingConfig {
            batch_interval: Duration::from_millis(10),
            ..SamplingConfig::default()
        };
        let sampling = Sampling::start(&config, &database, "test", &snapshot).unwrap();
        let sampler = sampling.sampler();
        let pruner = sampling.pruner.stats().clone();
        let reading = reading(&sampler, &pruner);
        assert_eq!(reading.queue_full, 0);
        assert_eq!(reading.queued, 0);
        assert_eq!(reading.body_budget_used, 0);
        let metrics = Metrics::new();
        metrics.publish_sampler(reading);
        assert!(metrics.render().contains("fastproxy_sample_queue_depth 0"));
        sampling.shutdown();
    }
}
