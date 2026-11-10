//! Active health checking. A [`HealthEngine`] keeps one prober task per
//! endpoint of every group with checks enabled, in step with the live
//! configuration: probers appear, restart or stop as snapshots change. Probe
//! results feed each endpoint's rise/fall state machine
//! ([`fast_proxy_balancing::EndpointRuntime::record_probe`]), and state
//! changes are published on the endpoint event bus.

mod engine;
mod probe;
mod prober;
mod reconciler;
mod status_gauges;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;

pub use engine::HealthEngine;
pub use probe::{run_probe, ProbeSpec};
pub use status_gauges::run_status_gauges;
