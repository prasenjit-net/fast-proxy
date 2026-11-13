//! Live traffic statistics: a sampler turns the proxy's cumulative counters
//! into rates every few seconds and pushes them to the UI.

use std::time::{Duration, Instant};

use fast_proxy_telemetry::{Counts, TrafficTotals};
use serde::Serialize;

use crate::events::Event;
use crate::state::SharedState;

/// How often the counters are sampled and a `traffic` event is pushed.
pub const SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// Request rate and error rate over one sampling window.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rates {
    pub requests_total: u64,
    pub requests_per_sec: f64,
    /// Share of the window's requests that got a 5xx response (0 to 1).
    pub error_rate: f64,
}

/// How many requests the sampler kept and lost in one window.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRates {
    pub captured_total: u64,
    pub captured_per_sec: f64,
    pub dropped_per_sec: f64,
}

/// Traffic for one subsystem.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubsystemTraffic {
    pub name: String,
    #[serde(flatten)]
    pub rates: Rates,
}

/// One sampling window's worth of live traffic.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficSnapshot {
    pub timestamp_ms: i64,
    pub window_secs: f64,
    pub total: Rates,
    pub active_connections: i64,
    pub active_requests: i64,
    pub samples: SampleRates,
    /// Sorted by name.
    pub subsystems: Vec<SubsystemTraffic>,
}

fn rates(previous: Counts, current: Counts, window_secs: f64) -> Rates {
    // Counters only grow, but a subsystem that vanished and came back would
    // not; saturate rather than go negative.
    let requests = current.requests.saturating_sub(previous.requests);
    let errors = current.errors.saturating_sub(previous.errors);
    Rates {
        requests_total: current.requests,
        requests_per_sec: if window_secs > 0.0 {
            requests as f64 / window_secs
        } else {
            0.0
        },
        error_rate: if requests > 0 {
            errors as f64 / requests as f64
        } else {
            0.0
        },
    }
}

/// Computes the rates between two readings taken `window` apart.
pub fn compute_snapshot(
    previous: &TrafficTotals,
    current: &TrafficTotals,
    window: Duration,
    timestamp_ms: i64,
) -> TrafficSnapshot {
    let window_secs = window.as_secs_f64();
    let mut subsystems: Vec<SubsystemTraffic> = current
        .per_subsystem
        .iter()
        .map(|(name, counts)| SubsystemTraffic {
            name: name.clone(),
            rates: rates(
                previous
                    .per_subsystem
                    .get(name)
                    .copied()
                    .unwrap_or_default(),
                *counts,
                window_secs,
            ),
        })
        .collect();
    subsystems.sort_by(|a, b| a.name.cmp(&b.name));
    TrafficSnapshot {
        timestamp_ms,
        window_secs,
        total: rates(previous.overall(), current.overall(), window_secs),
        active_connections: current.active_connections,
        active_requests: current.active_requests,
        samples: SampleRates {
            captured_total: current.samples_captured,
            captured_per_sec: per_sec(
                previous.samples_captured,
                current.samples_captured,
                window_secs,
            ),
            dropped_per_sec: per_sec(
                previous.samples_dropped,
                current.samples_dropped,
                window_secs,
            ),
        },
        subsystems,
    }
}

/// A counter's growth over the window, per second (a counter that went
/// backwards counts as no growth).
fn per_sec(previous: u64, current: u64, window_secs: f64) -> f64 {
    if window_secs > 0.0 {
        current.saturating_sub(previous) as f64 / window_secs
    } else {
        0.0
    }
}

/// Background sampler: stores the latest snapshot (for `GET /api/traffic`)
/// and pushes it to every WebSocket subscriber each interval.
pub fn spawn_sampler(state: SharedState) {
    tokio::spawn(async move {
        // The first sample comes after a full window: `interval` alone would
        // tick at once and report rates over a window of a few milliseconds.
        let mut interval = tokio::time::interval_at(
            tokio::time::Instant::now() + SAMPLE_INTERVAL,
            SAMPLE_INTERVAL,
        );
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut sampler = Sampler::start(&state);
        loop {
            interval.tick().await;
            sampler.sample(&state).await;
        }
    });
}

/// Remembers the previous reading so each sample can compute a delta.
struct Sampler {
    previous: TrafficTotals,
    last_tick: Instant,
}

impl Sampler {
    fn start(state: &SharedState) -> Self {
        Self {
            previous: state.metrics.traffic_totals(),
            last_tick: Instant::now(),
        }
    }

    /// Takes one reading, stores it as the latest snapshot, and pushes it to clients.
    async fn sample(&mut self, state: &SharedState) {
        let current = state.metrics.traffic_totals();
        let snapshot = compute_snapshot(
            &self.previous,
            &current,
            self.last_tick.elapsed(),
            chrono::Utc::now().timestamp_millis(),
        );
        self.previous = current;
        self.last_tick = Instant::now();

        *state.latest_traffic.write().await = Some(snapshot.clone());
        state.broadcast(Event::Traffic { data: snapshot });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(entries: &[(&str, u64, u64)]) -> TrafficTotals {
        TrafficTotals {
            per_subsystem: entries
                .iter()
                .map(|(name, requests, errors)| {
                    (
                        (*name).to_string(),
                        Counts {
                            requests: *requests,
                            errors: *errors,
                        },
                    )
                })
                .collect(),
            active_connections: 2,
            active_requests: 1,
            ..TrafficTotals::default()
        }
    }

    fn snapshot(previous: &[(&str, u64, u64)], current: &[(&str, u64, u64)]) -> TrafficSnapshot {
        compute_snapshot(
            &totals(previous),
            &totals(current),
            Duration::from_secs(2),
            1234,
        )
    }

    #[test]
    fn rates_are_the_window_delta_per_second() {
        let snap = snapshot(&[("shop", 100, 0)], &[("shop", 120, 0)]);
        assert_eq!(snap.total.requests_per_sec, 10.0);
        assert_eq!(snap.total.requests_total, 120);
        assert_eq!(snap.subsystems[0].rates.requests_per_sec, 10.0);
        assert_eq!(snap.window_secs, 2.0);
        assert_eq!(snap.timestamp_ms, 1234);
    }

    #[test]
    fn the_error_rate_is_the_share_of_the_windows_requests_that_failed() {
        let snap = snapshot(&[("shop", 100, 10)], &[("shop", 140, 20)]);
        assert_eq!(snap.total.error_rate, 0.25);
        assert_eq!(snap.subsystems[0].rates.error_rate, 0.25);
    }

    #[test]
    fn an_idle_window_has_zero_rates_not_nan() {
        let snap = snapshot(&[("shop", 100, 5)], &[("shop", 100, 5)]);
        assert_eq!(snap.total.requests_per_sec, 0.0);
        assert_eq!(snap.total.error_rate, 0.0);
    }

    #[test]
    fn subsystems_are_reported_separately_and_sorted_by_name() {
        let snap = snapshot(
            &[("blog", 0, 0), ("shop", 0, 0)],
            &[("shop", 40, 4), ("blog", 10, 0)],
        );
        let names: Vec<&str> = snap.subsystems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["blog", "shop"]);
        assert_eq!(snap.subsystems[0].rates.requests_per_sec, 5.0);
        assert_eq!(snap.subsystems[1].rates.error_rate, 0.1);
        assert_eq!(snap.total.requests_per_sec, 25.0);
    }

    #[test]
    fn a_subsystem_seen_for_the_first_time_counts_all_its_requests() {
        let snap = snapshot(&[], &[("shop", 10, 0)]);
        assert_eq!(snap.subsystems[0].rates.requests_per_sec, 5.0);
        assert_eq!(snap.total.requests_per_sec, 5.0);
    }

    #[test]
    fn the_sample_counts_are_per_second_over_the_window() {
        let mut previous = totals(&[("shop", 10, 0)]);
        previous.samples_captured = 4;
        previous.samples_dropped = 10;
        let mut current = totals(&[("shop", 20, 0)]);
        current.samples_captured = 10;
        current.samples_dropped = 30;
        let snap = compute_snapshot(&previous, &current, Duration::from_secs(2), 1);
        assert_eq!(snap.samples.captured_total, 10);
        assert_eq!(snap.samples.captured_per_sec, 3.0);
        assert_eq!(snap.samples.dropped_per_sec, 10.0);
        let json = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["samples"]["capturedPerSec"], 3.0);
    }

    #[test]
    fn sample_counters_that_went_backwards_do_not_go_negative() {
        let mut previous = totals(&[]);
        previous.samples_captured = 9;
        let snap = compute_snapshot(&previous, &totals(&[]), Duration::from_secs(2), 1);
        assert_eq!(snap.samples.captured_per_sec, 0.0);
    }

    #[test]
    fn a_counter_that_went_backwards_does_not_go_negative() {
        let snap = snapshot(&[("shop", 100, 5)], &[("shop", 10, 0)]);
        assert_eq!(snap.total.requests_per_sec, 0.0);
    }

    #[tokio::test]
    async fn sampling_stores_and_broadcasts_the_snapshot() {
        use std::time::Duration;

        let app = crate::test_support::TestApp::new().await;
        let mut events = app.state.events.subscribe();
        let mut sampler = Sampler::start(&app.state);
        assert!(app.state.latest_traffic.read().await.is_none());

        app.metrics
            .observe_request("shop", "/", 200, Duration::from_millis(1));
        app.metrics
            .observe_request("shop", "/", 503, Duration::from_millis(1));
        tokio::time::sleep(Duration::from_millis(50)).await;
        sampler.sample(&app.state).await;

        let stored = app.state.latest_traffic.read().await.clone().unwrap();
        assert!(stored.total.requests_per_sec > 0.0);
        assert_eq!(stored.total.requests_total, 2);
        assert_eq!(stored.total.error_rate, 0.5);
        assert_eq!(stored.subsystems[0].name, "shop");
        let Event::Traffic { data } = events.recv().await.unwrap() else {
            panic!("expected a traffic event");
        };
        assert_eq!(data, stored);
    }

    #[test]
    fn gauges_are_passed_through() {
        let snap = snapshot(&[], &[]);
        assert_eq!(snap.active_connections, 2);
        assert_eq!(snap.active_requests, 1);
        assert!(snap.subsystems.is_empty());
    }

    #[test]
    fn serializes_camel_case_with_the_rates_flattened_into_each_subsystem() {
        let value = serde_json::to_value(snapshot(&[], &[("shop", 10, 0)])).unwrap();
        assert!(value["timestampMs"].is_number());
        assert!(value["total"]["requestsPerSec"].is_number());
        assert_eq!(value["subsystems"][0]["name"], "shop");
        assert!(value["subsystems"][0]["requestsPerSec"].is_number());
        assert!(value["subsystems"][0]["errorRate"].is_number());
        assert!(value["activeConnections"].is_number());
    }
}
