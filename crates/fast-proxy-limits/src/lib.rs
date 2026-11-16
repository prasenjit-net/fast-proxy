//! Request limits with no I/O and no HTTP (M7, ADRs 0029 to 0031).
//!
//! [`Gcra`] decides one request from a stored time; [`KeyTable`] holds those
//! times for many keys within a memory budget shared by every table
//! ([`KeyBudget`]); [`RateLimitRuntime`] keeps a table per policy across
//! configuration swaps; [`KeyHasher`] turns a client address or a header value
//! into a key without storing either. [`bucket`] is the token bucket the sampler
//! uses. Time is always a parameter: [`monotonic_ns`] is the one clock callers
//! read. [`ConnectionCounter`] caps open connections in total and per peer, and
//! [`defaults`] is the one table of every process limit's default.

pub mod bucket;
mod clock;
mod connection_counter;
pub mod defaults;
mod gcra;
mod key_hasher;
mod key_table;
#[cfg(test)]
mod property_tests;
mod runtime;

pub use clock::monotonic_ns;
pub use connection_counter::{
    ConnectionCaps, ConnectionCounter, ConnectionPermit, ConnectionRefusal,
};
pub use gcra::{Gcra, Verdict};
pub use key_hasher::KeyHasher;
pub use key_table::{Admission, KeyBudget, KeyTable};
pub use runtime::{PolicyIdentity, RateLimitRuntime, DEFAULT_KEY_CAPACITY};
