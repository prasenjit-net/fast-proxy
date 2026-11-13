//! Sampling metrics. Every label is a value of a fixed set (the trigger names
//! and the drop reasons), never a request value, so cardinality is bounded by
//! the code.
//!
//! The data plane counts what it decides (a sample kept, or dropped by the
//! storm guard, the rate limit or the body budget). The sampler's own queue and
//! the database writer keep their counts elsewhere, and the pruner too; a
//! periodic task hands their absolute readings to [`SamplingMetrics::publish`],
//! which turns them into counter increments.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use prometheus::{IntCounter, IntCounterVec, IntGauge, Opts, Registry};

const REGISTRATION: &str = "static metric definitions register on a fresh registry";

/// Why a candidate sample was not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleDropReason {
    /// The same failure was just sampled (the storm guard).
    StormGuard,
    /// The instance's rate limit was spent.
    RateLimit,
    /// The hand-off queue was full.
    QueueFull,
    /// The body memory budget was spent (the sample is kept without bodies, so
    /// this counts bodies lost, not samples).
    Budget,
    /// The database could not take the batch.
    StoreError,
}

impl SampleDropReason {
    /// The label value.
    pub fn label(self) -> &'static str {
        match self {
            Self::StormGuard => "storm_guard",
            Self::RateLimit => "rate_limit",
            Self::QueueFull => "queue_full",
            Self::Budget => "budget",
            Self::StoreError => "store_error",
        }
    }
}

/// Absolute readings of the parts that keep their own counts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SamplerReading {
    pub queue_full: u64,
    pub store_error: u64,
    pub written: u64,
    /// Samples waiting in the queue now.
    pub queued: u64,
    /// Body bytes reserved now.
    pub body_budget_used: u64,
    /// Samples the pruner has deleted.
    pub pruned: u64,
    /// The shared table as the last prune pass left it.
    pub table_rows: u64,
    pub table_bytes: u64,
}

pub(crate) struct SamplingMetrics {
    captured: IntCounterVec,
    dropped: IntCounterVec,
    written: IntCounter,
    pruned: IntCounter,
    queue_depth: IntGauge,
    body_budget: IntGauge,
    table_rows: IntGauge,
    table_bytes: IntGauge,
    captured_total: AtomicU64,
    dropped_total: AtomicU64,
    last: Mutex<SamplerReading>,
}

fn counter(registry: &Registry, name: &str, help: &str) -> IntCounter {
    let counter = IntCounter::new(name, help).expect(REGISTRATION);
    registry
        .register(Box::new(counter.clone()))
        .expect(REGISTRATION);
    counter
}

fn gauge(registry: &Registry, name: &str, help: &str) -> IntGauge {
    let gauge = IntGauge::new(name, help).expect(REGISTRATION);
    registry
        .register(Box::new(gauge.clone()))
        .expect(REGISTRATION);
    gauge
}

fn counter_vec(registry: &Registry, name: &str, help: &str, label: &str) -> IntCounterVec {
    let vec = IntCounterVec::new(Opts::new(name, help), &[label]).expect(REGISTRATION);
    registry
        .register(Box::new(vec.clone()))
        .expect(REGISTRATION);
    vec
}

impl SamplingMetrics {
    pub(crate) fn new(registry: &Registry) -> Self {
        Self {
            captured: counter_vec(
                registry,
                "fastproxy_samples_captured_total",
                "Requests the sampler kept, by the trigger that matched first",
                "trigger",
            ),
            dropped: counter_vec(
                registry,
                "fastproxy_samples_dropped_total",
                "Samples that matched a trigger but were not kept, by reason",
                "reason",
            ),
            written: counter(
                registry,
                "fastproxy_samples_written_total",
                "Samples the database accepted",
            ),
            pruned: counter(
                registry,
                "fastproxy_sample_prune_deleted_total",
                "Samples deleted by retention",
            ),
            queue_depth: gauge(
                registry,
                "fastproxy_sample_queue_depth",
                "Samples waiting to be written",
            ),
            body_budget: gauge(
                registry,
                "fastproxy_sample_body_budget_bytes",
                "Body bytes held by requests that may be sampled",
            ),
            table_rows: gauge(
                registry,
                "fastproxy_sample_table_rows",
                "Rows in the shared sample table after the last prune pass",
            ),
            table_bytes: gauge(
                registry,
                "fastproxy_sample_table_bytes",
                "Bytes the shared sample table counts for after the last prune pass",
            ),
            captured_total: AtomicU64::new(0),
            dropped_total: AtomicU64::new(0),
            last: Mutex::new(SamplerReading::default()),
        }
    }

    pub(crate) fn captured(&self, trigger: &'static str) {
        self.captured.with_label_values(&[trigger]).inc();
        self.captured_total.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn dropped(&self, reason: SampleDropReason) {
        self.dropped.with_label_values(&[reason.label()]).inc();
        if reason != SampleDropReason::Budget {
            self.dropped_total.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Samples kept and samples lost, cumulative, for the live traffic view.
    pub(crate) fn totals(&self) -> (u64, u64) {
        (
            self.captured_total.load(Ordering::Relaxed),
            self.dropped_total.load(Ordering::Relaxed),
        )
    }

    /// Turns absolute readings into counter increments and gauge values.
    pub(crate) fn publish(&self, now: SamplerReading) {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let (queue_full, store_error) = (
            now.queue_full.saturating_sub(last.queue_full),
            now.store_error.saturating_sub(last.store_error),
        );
        self.dropped
            .with_label_values(&[SampleDropReason::QueueFull.label()])
            .inc_by(queue_full);
        self.dropped
            .with_label_values(&[SampleDropReason::StoreError.label()])
            .inc_by(store_error);
        self.dropped_total
            .fetch_add(queue_full + store_error, Ordering::Relaxed);
        self.written
            .inc_by(now.written.saturating_sub(last.written));
        self.pruned.inc_by(now.pruned.saturating_sub(last.pruned));
        let gauge_value = |value: u64| i64::try_from(value).unwrap_or(i64::MAX);
        self.queue_depth.set(gauge_value(now.queued));
        self.body_budget.set(gauge_value(now.body_budget_used));
        self.table_rows.set(gauge_value(now.table_rows));
        self.table_bytes.set(gauge_value(now.table_bytes));
        *last = now;
    }
}

#[cfg(test)]
mod tests {
    use crate::Metrics;

    use super::*;

    fn rendered(metrics: &Metrics) -> String {
        metrics.render()
    }

    #[test]
    fn captured_and_dropped_samples_are_counted_by_their_fixed_labels() {
        let metrics = Metrics::new();
        metrics.sample_captured("upstream_failure");
        metrics.sample_captured("upstream_failure");
        metrics.sample_captured("slow");
        metrics.sample_dropped(SampleDropReason::StormGuard);
        metrics.sample_dropped(SampleDropReason::RateLimit);
        let text = rendered(&metrics);
        assert!(text.contains("fastproxy_samples_captured_total{trigger=\"upstream_failure\"} 2"));
        assert!(text.contains("fastproxy_samples_captured_total{trigger=\"slow\"} 1"));
        assert!(text.contains("fastproxy_samples_dropped_total{reason=\"storm_guard\"} 1"));
        assert!(text.contains("fastproxy_samples_dropped_total{reason=\"rate_limit\"} 1"));
        assert_eq!(metrics.sample_totals(), (3, 2));
    }

    #[test]
    fn every_drop_reason_has_a_distinct_label() {
        let labels: Vec<&str> = [
            SampleDropReason::StormGuard,
            SampleDropReason::RateLimit,
            SampleDropReason::QueueFull,
            SampleDropReason::Budget,
            SampleDropReason::StoreError,
        ]
        .into_iter()
        .map(SampleDropReason::label)
        .collect();
        let mut unique = labels.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), labels.len());
    }

    #[test]
    fn a_lost_body_is_counted_but_is_not_a_lost_sample() {
        let metrics = Metrics::new();
        metrics.sample_dropped(SampleDropReason::Budget);
        assert!(rendered(&metrics).contains("fastproxy_samples_dropped_total{reason=\"budget\"} 1"));
        assert_eq!(metrics.sample_totals(), (0, 0));
    }

    #[test]
    fn readings_become_increments_not_totals() {
        let metrics = Metrics::new();
        let first = SamplerReading {
            queue_full: 3,
            store_error: 2,
            written: 10,
            queued: 4,
            body_budget_used: 4096,
            pruned: 7,
            table_rows: 1000,
            table_bytes: 2_000_000,
        };
        metrics.publish_sampler(first);
        metrics.publish_sampler(SamplerReading {
            queue_full: 5,
            written: 25,
            queued: 0,
            body_budget_used: 0,
            pruned: 7,
            ..first
        });
        let text = rendered(&metrics);
        assert!(
            text.contains("fastproxy_samples_dropped_total{reason=\"queue_full\"} 5"),
            "{text}"
        );
        assert!(text.contains("fastproxy_samples_dropped_total{reason=\"store_error\"} 2"));
        assert!(text.contains("fastproxy_samples_written_total 25"));
        assert!(text.contains("fastproxy_sample_prune_deleted_total 7"));
        assert!(text.contains("fastproxy_sample_queue_depth 0"));
        assert!(text.contains("fastproxy_sample_body_budget_bytes 0"));
        assert!(text.contains("fastproxy_sample_table_rows 1000"));
        assert!(text.contains("fastproxy_sample_table_bytes 2000000"));
        assert_eq!(
            metrics.sample_totals(),
            (0, 7),
            "queue-full and store-error losses are in the total"
        );
    }

    #[test]
    fn a_reading_that_went_backwards_never_makes_a_counter_negative() {
        let metrics = Metrics::new();
        metrics.publish_sampler(SamplerReading {
            written: 10,
            ..SamplerReading::default()
        });
        metrics.publish_sampler(SamplerReading::default());
        assert!(rendered(&metrics).contains("fastproxy_samples_written_total 10"));
    }

    #[test]
    fn no_label_can_come_from_a_request() {
        // The only labels are `trigger` and `reason`, and the methods that set
        // them take a `&'static str` and an enum.
        let metrics = Metrics::new();
        metrics.sample_captured("baseline");
        let text = rendered(&metrics);
        for line in text.lines().filter(|l| l.starts_with("fastproxy_sample")) {
            assert!(
                !line.contains("subsystem") && !line.contains("route") && !line.contains("client"),
                "{line}"
            );
        }
    }
}
