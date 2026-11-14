//! Where finished batches go. The sample service implements this over the
//! database; the sampler only knows the trait, so it stays free of storage.

use std::fmt;

use crate::sample::Sample;

/// Why a batch could not be written (busy or full database, I/O). The detail is
/// for logs and is never shown to API callers.
#[derive(Debug)]
pub struct SinkError(pub String);

impl fmt::Display for SinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SinkError {}

/// Takes batches of redacted samples. `write_batch` may block (it runs on the
/// sampler's own thread) and must be all-or-nothing.
pub trait SampleSink: Send + 'static {
    fn write_batch(&mut self, batch: Vec<Sample>) -> Result<(), SinkError>;
}
