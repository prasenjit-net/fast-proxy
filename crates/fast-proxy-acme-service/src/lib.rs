//! The ACME runner (M4): the part of ACME that does I/O. One runs per
//! instance; only the one holding the renewal lease talks to the CA. Every
//! decision comes from the pure `fast-proxy-acme` policy, and every write goes
//! through the `ConfigService` (challenges and certificates as revisions; lease,
//! status and account without one).

mod order;
mod runner;
mod self_check;
mod settings;

pub use runner::{AcmeControl, AcmeRunner};
pub use settings::{system_clock, Clock, RunnerSettings, SelfCheck};
