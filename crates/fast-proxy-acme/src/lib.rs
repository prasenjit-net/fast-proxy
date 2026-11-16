//! ACME for fast-proxy (M4). Apart from the protocol client ([`AcmeClient`],
//! a thin adapter over `instant-acme`), this crate decides and does no I/O:
//!
//! - [`eligible_names`] chooses which hosts of a subsystem go into its
//!   certificate and why the others do not (K5).
//! - [`RenewalPolicy`] says when a certificate is due, how long to back off
//!   after a failure and when the hourly cap stops further orders (K6, K7).
//! - [`classify_problem`] turns a CA's problem type into a failure kind.
//! - [`OrderStep`] is the state machine of one order, so the runner that talks
//!   to the CA is a thin loop over pure transitions.
//!
//! Time is always a parameter (`now_ms`, wall clock milliseconds), so tests
//! drive every rule with a fake clock.

mod client;
mod eligibility;
mod order_step;
mod problem;
mod renewal;

pub use client::{AcmeClient, AcmeError, AcmeSession, HttpChallenge, PendingOrder};
pub use eligibility::{eligible_names, Eligibility};
pub use order_step::{OrderEvent, OrderStep, TransitionError};
pub use problem::{classify_problem, FailureKind};
pub use renewal::{CertificateFacts, Decision, OrderReason, RenewalFacts, RenewalPolicy};

#[cfg(test)]
mod property_tests;
