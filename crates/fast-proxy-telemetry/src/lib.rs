//! Access log, metrics, and live traffic statistics shared by both planes.

mod access_log;

pub use access_log::{AccessLog, AccessRecord};
