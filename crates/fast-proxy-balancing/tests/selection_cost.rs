//! Selection cost per algorithm (plan task B8). Timing depends on the machine,
//! so the measurement is `#[ignore]`d; run it in release mode:
//!
//! `cargo test -p fast-proxy-balancing --release --test selection_cost -- --ignored --nocapture`
//!
//! The always-on test only checks the hot path does not blow the budget by a
//! wide margin even in a debug build.

use std::sync::Arc;
use std::time::Instant;

use fast_proxy_balancing::{Balancer, Candidate, EndpointRuntime, RequestContext, Rng};
use fast_proxy_model::{EndpointId, FailurePolicy, HashKey, LbAlgorithm, LbPolicy};

const ALGORITHMS: [LbAlgorithm; 5] = [
    LbAlgorithm::RoundRobin,
    LbAlgorithm::WeightedRoundRobin,
    LbAlgorithm::LeastConnections,
    LbAlgorithm::P2cEwma,
    LbAlgorithm::ConsistentHash,
];

fn nanos_per_pick(algorithm: LbAlgorithm, endpoints: usize, picks: u32) -> f64 {
    let candidates: Vec<Candidate> = (0..endpoints)
        .map(|_| Candidate {
            runtime: Arc::new(EndpointRuntime::new()),
            weight: 1,
            admin_enabled: true,
        })
        .collect();
    let ids: Vec<_> = (0..endpoints).map(|i| (EndpointId(i as i64), 1)).collect();
    let lb = LbPolicy {
        algorithm,
        hash_key: HashKey::ClientIp,
    };
    let balancer = Balancer::new(&lb, FailurePolicy::FailOpen, &ids);
    let mut rng = Rng::seeded(1);
    let started = Instant::now();
    for i in 0..picks {
        let ctx = RequestContext {
            key_hash: Some(u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        };
        let picked = balancer.select(&candidates, &ctx, None, &mut rng, 0);
        std::hint::black_box(picked);
    }
    started.elapsed().as_nanos() as f64 / f64::from(picks)
}

#[test]
#[ignore = "timing measurement; run in release mode"]
fn report_selection_cost() {
    for algorithm in ALGORITHMS {
        for endpoints in [3, 64] {
            let nanos = nanos_per_pick(algorithm, endpoints, 2_000_000);
            println!(
                "{:<22} {endpoints:>3} endpoints: {nanos:>8.1} ns/pick",
                algorithm.as_str()
            );
        }
    }
}

#[test]
fn selection_is_cheap_even_unoptimized() {
    for algorithm in ALGORITHMS {
        let nanos = nanos_per_pick(algorithm, 3, 20_000);
        assert!(nanos < 20_000.0, "{algorithm:?} took {nanos} ns per pick");
    }
}
