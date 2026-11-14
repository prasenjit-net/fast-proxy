//! The hand-off from the request path to the sink (decision J10). The data
//! plane holds a cheap [`Sampler`] handle and calls [`Sampler::try_submit`],
//! which never blocks: a full queue drops the sample and counts it. A thread of
//! its own redacts, batches and writes through the [`SampleSink`]. A failed
//! write drops that batch, counts it, and backs off, so a sick database is
//! never retried in a tight loop and never held in memory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::batcher::Batcher;
use crate::budget::BodyBudget;
use crate::raw::RawSample;
use crate::redact::Redactor;
use crate::sample::Sample;
use crate::sink::SampleSink;

const FIRST_BACKOFF_MS: u64 = 250;
const MAX_BACKOFF_MS: u64 = 8_000;

/// Process settings of the sampler (`[sampling]` in the config file).
#[derive(Debug, Clone)]
pub struct SamplerConfig {
    pub queue_depth: usize,
    pub batch_size: usize,
    pub batch_interval: Duration,
    pub body_budget_bytes: u64,
    /// Stamped on every sample.
    pub instance: String,
}

/// Counters the metrics read.
#[derive(Debug, Default)]
pub struct SamplerStats {
    queue_full: AtomicU64,
    store_error: AtomicU64,
    written: AtomicU64,
    queued: AtomicU64,
}

/// A reading of [`SamplerStats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatsReading {
    /// Samples refused because the queue was full.
    pub queue_full: u64,
    /// Samples lost because a batch could not be written, or arrived while the
    /// writer was backing off.
    pub store_error: u64,
    /// Samples the sink accepted.
    pub written: u64,
    /// Samples waiting in the queue now.
    pub queued: u64,
}

impl SamplerStats {
    pub fn read(&self) -> StatsReading {
        StatsReading {
            queue_full: self.queue_full.load(Ordering::Relaxed),
            store_error: self.store_error.load(Ordering::Relaxed),
            written: self.written.load(Ordering::Relaxed),
            queued: self.queued.load(Ordering::Relaxed),
        }
    }
}

/// Why a sample was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    QueueFull,
    /// The sampler has been shut down.
    Closed,
}

enum Message {
    Sample(Box<RawSample>),
    Stop,
}

/// The handle the data plane holds. Clone it freely.
#[derive(Clone)]
pub struct Sampler {
    queue: SyncSender<Message>,
    budget: Arc<BodyBudget>,
    stats: Arc<SamplerStats>,
    redactor: Arc<RwLock<Arc<Redactor>>>,
}

impl Sampler {
    /// Queues a sample without blocking. A refused sample is dropped (and its
    /// body reservation returned) and counted.
    pub fn try_submit(&self, raw: RawSample) -> Result<(), SubmitError> {
        match self.queue.try_send(Message::Sample(Box::new(raw))) {
            Ok(()) => {
                self.stats.queued.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(TrySendError::Full(_)) => {
                self.stats.queue_full.fetch_add(1, Ordering::Relaxed);
                Err(SubmitError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Closed),
        }
    }

    /// The memory budget teed bodies reserve from.
    pub fn budget(&self) -> &Arc<BodyBudget> {
        &self.budget
    }

    pub fn stats(&self) -> &Arc<SamplerStats> {
        &self.stats
    }

    /// Replaces the redactor, for a settings change; samples already queued are
    /// redacted with the new one when they are reached.
    pub fn set_redactor(&self, redactor: Redactor) {
        if let Ok(mut current) = self.redactor.write() {
            *current = Arc::new(redactor);
        }
    }
}

/// The running sampler: its handle and its thread.
pub struct SamplerRuntime {
    sampler: Sampler,
    thread: JoinHandle<()>,
}

impl SamplerRuntime {
    /// Starts the sampler's thread; fails only if the operating system cannot
    /// start one.
    pub fn start(
        config: SamplerConfig,
        redactor: Redactor,
        sink: impl SampleSink,
    ) -> std::io::Result<Self> {
        let (queue, inbox) = mpsc::sync_channel(config.queue_depth.max(1));
        let sampler = Sampler {
            queue,
            budget: BodyBudget::new(config.body_budget_bytes),
            stats: Arc::new(SamplerStats::default()),
            redactor: Arc::new(RwLock::new(Arc::new(redactor))),
        };
        let worker = Worker {
            inbox,
            stats: sampler.stats.clone(),
            redactor: sampler.redactor.clone(),
            sink: Box::new(sink),
            config,
        };
        let thread = std::thread::Builder::new()
            .name("fast-proxy-sampler".into())
            .spawn(move || worker.run())?;
        Ok(Self { sampler, thread })
    }

    pub fn handle(&self) -> Sampler {
        self.sampler.clone()
    }

    /// Stops the thread after it has handled everything queued before this call
    /// and written the last partial batch.
    pub fn shutdown(self) {
        // The queue may be full; this waits for room, which is fine at shutdown.
        let _ = self.sampler.queue.send(Message::Stop);
        let _ = self.thread.join();
    }
}

struct Worker {
    inbox: mpsc::Receiver<Message>,
    stats: Arc<SamplerStats>,
    redactor: Arc<RwLock<Arc<Redactor>>>,
    sink: Box<dyn SampleSink>,
    config: SamplerConfig,
}

/// Waiting out a failed write.
struct Backoff {
    until_ms: u64,
    next_step_ms: u64,
}

impl Worker {
    fn run(mut self) {
        let started = Instant::now();
        let now = || started.elapsed().as_millis() as u64;
        let interval_ms = self.config.batch_interval.as_millis() as u64;
        let mut batcher = Batcher::new(self.config.batch_size, interval_ms);
        let mut backoff = Backoff {
            until_ms: 0,
            next_step_ms: FIRST_BACKOFF_MS,
        };
        loop {
            let message = match batcher.deadline_ms() {
                Some(deadline) => self
                    .inbox
                    .recv_timeout(Duration::from_millis(deadline.saturating_sub(now())))
                    .map_err(|err| matches!(err, RecvTimeoutError::Disconnected)),
                None => self.inbox.recv().map_err(|_| true),
            };
            match message {
                Ok(Message::Sample(raw)) => {
                    self.stats.queued.fetch_sub(1, Ordering::Relaxed);
                    let sample = self.redact(*raw);
                    if let Some(batch) = batcher.push(sample, now()) {
                        self.write(batch, &mut backoff, now());
                    }
                }
                Ok(Message::Stop) | Err(true) => {
                    if let Some(batch) = batcher.drain() {
                        self.write(batch, &mut backoff, now());
                    }
                    return;
                }
                Err(false) => {
                    if let Some(batch) = batcher.due(now()) {
                        self.write(batch, &mut backoff, now());
                    }
                }
            }
        }
    }

    fn redact(&self, raw: RawSample) -> Sample {
        let redactor = self
            .redactor
            .read()
            .map(|current| current.clone())
            .unwrap_or_default();
        Sample::from_raw(raw, &redactor, &self.config.instance)
    }

    fn write(&mut self, batch: Vec<Sample>, backoff: &mut Backoff, now_ms: u64) {
        let count = batch.len() as u64;
        if now_ms < backoff.until_ms {
            self.stats.store_error.fetch_add(count, Ordering::Relaxed);
            return;
        }
        match self.sink.write_batch(batch) {
            Ok(()) => {
                self.stats.written.fetch_add(count, Ordering::Relaxed);
                backoff.next_step_ms = FIRST_BACKOFF_MS;
            }
            Err(_) => {
                self.stats.store_error.fetch_add(count, Ordering::Relaxed);
                backoff.until_ms = now_ms + backoff.next_step_ms;
                backoff.next_step_ms = (backoff.next_step_ms * 2).min(MAX_BACKOFF_MS);
            }
        }
    }
}
