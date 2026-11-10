//! Runtime state and endpoint selection for upstream groups. Pure: no I/O and
//! no clock of its own (callers pass `now` in milliseconds), so every state
//! machine is deterministic under test.
//!
//! - [`EndpointRuntime`] holds an endpoint's atomics (in-flight, health,
//!   ejection, latency) and is carried across configuration swaps by the
//!   [`RuntimeRegistry`].
//! - [`Balancer`] picks an endpoint per request with the group's algorithm,
//!   honouring eligibility and the failure policy.
//! - [`record_outcome`] is the passive health state machine.

mod algorithms;
mod balancer;
mod candidate;
mod clock;
mod hashing;
mod passive;
mod registry;
mod retry_budget;
mod rng;
mod runtime;

pub use balancer::Balancer;
pub use candidate::{Candidate, RequestContext, Selection};
pub use clock::monotonic_ms;
pub use hashing::{hash_bytes, hash_ip};
pub use passive::{record_outcome, Outcome, Transition};
pub use registry::RuntimeRegistry;
pub use retry_budget::RetryBudget;
pub use rng::Rng;
pub use runtime::{EndpointRuntime, InFlightGuard, RuntimeStatus};

#[cfg(test)]
mod test_support;
