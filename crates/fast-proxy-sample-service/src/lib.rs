//! Samples in the shared database (ADR 0022): [`SampleWriter`] is the sampler's
//! sink, [`Pruner`] keeps the table within its retention, and [`SampleReader`]
//! is what the admin plane reads. Samples are telemetry, not configuration:
//! nothing here records a revision, so a sample never makes another instance
//! reload. Each part opens its own connection to the database, so none of them
//! queues behind a configuration write.

mod convert;
mod pruner;
mod reader;
mod writer;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use pruner::{prune_once, PruneReport, Pruner, PrunerConfig, PrunerReading, PrunerStats};
pub use reader::SampleReader;
pub use writer::{SampleWriter, WRITE_LOCK_TIMEOUT};
