use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use fast_proxy_model::{SamplingSettings, Trigger};

use super::*;
use crate::facts::{Answer, AttemptLog, Outcome, RequestHead, UnusualFlags};

/// A generous limit for waiting on an event; tests never sleep a fixed time.
const PATIENCE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct SinkState {
    batches: Mutex<Vec<Vec<Sample>>>,
    calls: AtomicUsize,
    fail: AtomicBool,
    /// While true, `write_batch` blocks, so the queue can be filled.
    gate: Mutex<bool>,
    gate_open: Condvar,
}

#[derive(Clone)]
struct FakeSink(Arc<SinkState>);

impl SampleSink for FakeSink {
    fn write_batch(&mut self, batch: Vec<Sample>) -> Result<(), SinkError> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        let mut closed = self.0.gate.lock().unwrap();
        while *closed {
            closed = self.0.gate_open.wait(closed).unwrap();
        }
        drop(closed);
        if self.0.fail.load(Ordering::SeqCst) {
            return Err(SinkError("database is locked".into()));
        }
        self.0.batches.lock().unwrap().push(batch);
        Ok(())
    }
}

impl SinkState {
    fn delivered(&self) -> usize {
        self.batches.lock().unwrap().iter().map(Vec::len).sum()
    }

    fn close_gate(&self) {
        *self.gate.lock().unwrap() = true;
    }

    fn open_gate(&self) {
        *self.gate.lock().unwrap() = false;
        self.gate_open.notify_all();
    }
}

fn config(batch_size: usize, interval: Duration, queue_depth: usize) -> SamplerConfig {
    SamplerConfig {
        queue_depth,
        batch_size,
        batch_interval: interval,
        body_budget_bytes: 1_000_000,
        instance: "test-instance".into(),
    }
}

fn raw(n: u32, budget: Option<&Arc<BodyBudget>>) -> RawSample {
    RawSample {
        at_ms: i64::from(n),
        request_id: format!("req-{n}"),
        triggers: [Trigger::Slow].into_iter().collect(),
        subsystem: "shop".into(),
        route: "/".into(),
        head: RequestHead {
            method: "GET".into(),
            path: "/".into(),
            query: Some("token=ZQ".into()),
            headers: vec![("authorization".into(), "Bearer ZQ".into())],
            flags: UnusualFlags::default(),
            client: "203.0.113.9:1".parse().unwrap(),
        },
        outcome: Outcome {
            status: 200,
            elapsed_ms: 3000,
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
        reservation: budget.and_then(|b| b.reserve(100)),
    }
}

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let started = Instant::now();
    while !condition() {
        assert!(started.elapsed() < PATIENCE, "timed out waiting for {what}");
        std::thread::yield_now();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn start(
    batch_size: usize,
    interval: Duration,
    queue_depth: usize,
) -> (SamplerRuntime, Arc<SinkState>) {
    let state = Arc::new(SinkState::default());
    let runtime = SamplerRuntime::start(
        config(batch_size, interval, queue_depth),
        Redactor::default(),
        FakeSink(state.clone()),
    )
    .unwrap();
    (runtime, state)
}

const LONG: Duration = Duration::from_secs(3600);

#[test]
fn samples_are_redacted_stamped_and_delivered_in_full_batches() {
    let (runtime, sink) = start(3, LONG, 64);
    let sampler = runtime.handle();
    for n in 1..=7 {
        sampler.try_submit(raw(n, None)).unwrap();
    }
    wait_until("two full batches", || {
        sink.batches.lock().unwrap().len() == 2
    });
    {
        let batches = sink.batches.lock().unwrap();
        assert_eq!(batches[0].len(), 3);
        assert_eq!(batches[1].len(), 3);
        assert_eq!(batches[0][0].request_id, "req-1", "order is preserved");
        assert_eq!(batches[0][0].instance, "test-instance");
        assert_eq!(
            batches[0][0].request.query.as_deref(),
            Some("token=[redacted]")
        );
        assert_eq!(batches[0][0].request.headers[0].1, "[redacted]");
    }
    runtime.shutdown();
    assert_eq!(
        sink.delivered(),
        7,
        "the partial batch is flushed at shutdown"
    );
}

#[test]
fn a_partial_batch_is_written_when_its_interval_has_passed() {
    let (runtime, sink) = start(100, Duration::from_millis(20), 64);
    runtime.handle().try_submit(raw(1, None)).unwrap();
    wait_until("the interval flush", || sink.delivered() == 1);
    assert_eq!(sink.batches.lock().unwrap().len(), 1);
    runtime.shutdown();
}

#[test]
fn shutdown_loses_nothing_that_was_accepted() {
    let (runtime, sink) = start(100, LONG, 64);
    let sampler = runtime.handle();
    for n in 0..10 {
        sampler.try_submit(raw(n, None)).unwrap();
    }
    runtime.shutdown();
    assert_eq!(sink.delivered(), 10);
    assert_eq!(sampler.stats().read().written, 10);
    assert_eq!(sampler.try_submit(raw(99, None)), Err(SubmitError::Closed));
}

#[test]
fn a_full_queue_drops_the_sample_counts_it_and_returns_its_budget() {
    let (runtime, sink) = start(1, LONG, 2);
    let sampler = runtime.handle();
    sink.close_gate();
    let mut accepted = 0;
    let mut refused = 0;
    for n in 0..20 {
        match sampler.try_submit(raw(n, Some(sampler.budget()))) {
            Ok(()) => accepted += 1,
            Err(SubmitError::QueueFull) => refused += 1,
            Err(other) => panic!("{other:?}"),
        }
    }
    assert!(
        refused >= 15,
        "the writer is stuck, so only a few fit: {accepted} accepted"
    );
    assert_eq!(sampler.stats().read().queue_full, refused);
    // The writer takes one sample and blocks in the sink; from then on nothing moves.
    wait_until("the writer to block in the sink", || {
        sink.calls.load(Ordering::SeqCst) == 1
    });
    let waiting = sampler.stats().read().queued;
    assert_eq!(
        sampler.budget().used(),
        waiting * 100,
        "only the samples still in the queue hold a reservation; a refused one gave its back at once"
    );
    sink.open_gate();
    runtime.shutdown();
    assert_eq!(sink.delivered() as u64, accepted);
    assert_eq!(
        sampler.budget().used(),
        0,
        "and every accepted one after it was handled"
    );
}

#[test]
fn a_failing_sink_drops_the_batch_counts_it_and_backs_off() {
    let (runtime, sink) = start(1, LONG, 64);
    let sampler = runtime.handle();
    sink.fail.store(true, Ordering::SeqCst);
    sampler.try_submit(raw(1, Some(sampler.budget()))).unwrap();
    wait_until("the first failure", || {
        sampler.stats().read().store_error == 1
    });
    assert_eq!(sink.calls.load(Ordering::SeqCst), 1);
    // Samples that arrive while the writer is backing off are dropped without
    // calling the sink at all.
    for n in 2..=6 {
        sampler.try_submit(raw(n, Some(sampler.budget()))).unwrap();
    }
    wait_until("the dropped samples", || {
        sampler.stats().read().store_error == 6
    });
    assert_eq!(sink.calls.load(Ordering::SeqCst), 1, "no tight retry loop");
    assert_eq!(sampler.stats().read().written, 0);
    runtime.shutdown();
    assert_eq!(
        sampler.budget().used(),
        0,
        "the failure path released every reservation"
    );
}

#[test]
fn the_writer_recovers_once_the_sink_does_and_the_backoff_has_passed() {
    let (runtime, sink) = start(1, LONG, 64);
    let sampler = runtime.handle();
    sink.fail.store(true, Ordering::SeqCst);
    sampler.try_submit(raw(1, None)).unwrap();
    wait_until("the failure", || sampler.stats().read().store_error == 1);
    sink.fail.store(false, Ordering::SeqCst);
    let mut n = 2;
    wait_until("a write after the backoff", || {
        let _ = sampler.try_submit(raw(n, None));
        n += 1;
        sampler.stats().read().written > 0
    });
    runtime.shutdown();
}

#[test]
fn the_queue_depth_reading_follows_what_is_waiting() {
    let (runtime, sink) = start(1, LONG, 8);
    let sampler = runtime.handle();
    sink.close_gate();
    for n in 0..4 {
        sampler.try_submit(raw(n, None)).unwrap();
    }
    wait_until("the writer to take one", || {
        sampler.stats().read().queued == 3
    });
    sink.open_gate();
    wait_until("the queue to drain", || sampler.stats().read().queued == 0);
    runtime.shutdown();
}

#[test]
fn a_changed_redactor_applies_to_later_samples() {
    let (runtime, sink) = start(1, LONG, 64);
    let sampler = runtime.handle();
    let with_addition = SamplingSettings {
        redact_query_params: vec!["page".into()],
        ..SamplingSettings::default()
    };
    sampler.try_submit(raw(1, None)).unwrap();
    wait_until("the first sample", || sink.delivered() == 1);
    sampler.set_redactor(Redactor::new(&with_addition, &[]));
    let mut second = raw(2, None);
    second.head.query = Some("page=3".into());
    sampler.try_submit(second).unwrap();
    wait_until("the second sample", || sink.delivered() == 2);
    let batches = sink.batches.lock().unwrap();
    assert_eq!(
        batches[1][0].request.query.as_deref(),
        Some("page=[redacted]")
    );
    drop(batches);
    runtime.shutdown();
}

#[test]
fn the_handle_is_cheap_to_clone_and_shares_one_queue_and_budget() {
    let (runtime, sink) = start(2, LONG, 64);
    let (a, b) = (runtime.handle(), runtime.handle());
    a.try_submit(raw(1, None)).unwrap();
    b.try_submit(raw(2, None)).unwrap();
    wait_until("both samples", || sink.delivered() == 2);
    assert!(Arc::ptr_eq(a.budget(), b.budget()));
    assert!(Arc::ptr_eq(a.stats(), b.stats()));
    runtime.shutdown();
}
