use std::sync::atomic::Ordering::Relaxed;

use fast_proxy_model::{EndpointId, FailurePolicy, HashKey, LbAlgorithm, LbPolicy};

use super::*;
use crate::test_support::candidates;

const ALGORITHMS: [LbAlgorithm; 5] = [
    LbAlgorithm::RoundRobin,
    LbAlgorithm::WeightedRoundRobin,
    LbAlgorithm::LeastConnections,
    LbAlgorithm::P2cEwma,
    LbAlgorithm::ConsistentHash,
];

fn balancer(algorithm: LbAlgorithm, failure: FailurePolicy, endpoints: &[Candidate]) -> Balancer {
    let ids: Vec<_> = endpoints
        .iter()
        .enumerate()
        .map(|(i, c)| (EndpointId(i as i64 + 1), c.weight))
        .collect();
    let lb = LbPolicy {
        algorithm,
        hash_key: HashKey::ClientIp,
    };
    Balancer::new(&lb, failure, &ids)
}

fn make_unhealthy(candidate: &Candidate) {
    candidate.runtime.record_probe(false, 1, 1);
}

fn eject(candidate: &Candidate, until: u64) {
    candidate.runtime.ejected_until_ms.store(until, Relaxed);
}

fn ctx(key: u64) -> RequestContext {
    RequestContext {
        key_hash: Some(key),
    }
}

#[test]
fn an_ineligible_endpoint_is_never_chosen_while_an_eligible_one_exists() {
    let mut rng = Rng::seeded(11);
    for algorithm in ALGORITHMS {
        for failure in [FailurePolicy::FailOpen, FailurePolicy::FailClosed] {
            for _ in 0..300 {
                let size = 1 + rng.below(6);
                let weights: Vec<u32> = (0..size).map(|_| 1 + rng.below(5) as u32).collect();
                let endpoints = candidates(&weights);
                let mut any_eligible = false;
                let endpoints: Vec<Candidate> = endpoints
                    .into_iter()
                    .map(|mut endpoint| {
                        endpoint.admin_enabled = rng.below(10) < 7;
                        match rng.below(3) {
                            0 => make_unhealthy(&endpoint),
                            1 => eject(&endpoint, 1_000),
                            _ => {}
                        }
                        any_eligible |= endpoint.admin_enabled && endpoint.runtime.is_routable(10);
                        endpoint
                    })
                    .collect();
                let balancer = balancer(algorithm, failure, &endpoints);
                let picked = balancer.select(&endpoints, &ctx(rng.next_u64()), None, &mut rng, 10);
                if any_eligible {
                    let selection = picked.expect("an eligible endpoint exists");
                    assert!(!selection.panic);
                    assert!(endpoints[selection.index].is_eligible(10), "{algorithm:?}");
                } else if let Some(selection) = picked {
                    assert_eq!(failure, FailurePolicy::FailOpen);
                    assert!(selection.panic);
                    assert!(endpoints[selection.index].admin_enabled, "{algorithm:?}");
                }
            }
        }
    }
}

#[test]
fn fail_open_serves_from_admin_enabled_endpoints_when_none_is_healthy() {
    for algorithm in ALGORITHMS {
        let endpoints = candidates(&[1, 1]);
        endpoints.iter().for_each(make_unhealthy);
        let balancer = balancer(algorithm, FailurePolicy::FailOpen, &endpoints);
        let selection = balancer
            .select(&endpoints, &ctx(7), None, &mut Rng::seeded(1), 0)
            .unwrap();
        assert!(selection.panic, "{algorithm:?}");
    }
}

#[test]
fn fail_closed_returns_none_when_none_is_healthy() {
    for algorithm in ALGORITHMS {
        let endpoints = candidates(&[1, 1]);
        endpoints.iter().for_each(make_unhealthy);
        let balancer = balancer(algorithm, FailurePolicy::FailClosed, &endpoints);
        assert!(balancer
            .select(&endpoints, &ctx(7), None, &mut Rng::seeded(1), 0)
            .is_none());
    }
}

#[test]
fn draining_and_disabled_endpoints_are_never_used_even_in_panic_mode() {
    for algorithm in ALGORITHMS {
        let mut endpoints = candidates(&[1, 1]);
        endpoints.iter_mut().for_each(|e| e.admin_enabled = false);
        let balancer = balancer(algorithm, FailurePolicy::FailOpen, &endpoints);
        assert!(balancer
            .select(&endpoints, &ctx(7), None, &mut Rng::seeded(1), 0)
            .is_none());
    }
}

#[test]
fn the_excluded_endpoint_is_skipped_and_a_lone_endpoint_yields_none() {
    for algorithm in ALGORITHMS {
        let endpoints = candidates(&[1, 1, 1]);
        let balancer = balancer(algorithm, FailurePolicy::FailOpen, &endpoints);
        let mut rng = Rng::seeded(3);
        for key in 0..200 {
            let selection = balancer
                .select(&endpoints, &ctx(key * 7919), Some(1), &mut rng, 0)
                .unwrap();
            assert_ne!(selection.index, 1, "{algorithm:?}");
        }
        let lone = candidates(&[1]);
        let balancer = self::balancer(algorithm, FailurePolicy::FailOpen, &lone);
        assert!(balancer
            .select(&lone, &ctx(1), Some(0), &mut Rng::seeded(1), 0)
            .is_none());
    }
}

#[test]
fn consistent_hash_is_sticky_per_key_and_falls_back_to_rotation_without_one() {
    let endpoints = candidates(&[1, 1, 1]);
    let balancer = balancer(
        LbAlgorithm::ConsistentHash,
        FailurePolicy::FailOpen,
        &endpoints,
    );
    assert_eq!(balancer.hash_key(), Some(&HashKey::ClientIp));
    let mut rng = Rng::seeded(1);
    let first = balancer
        .select(&endpoints, &ctx(99), None, &mut rng, 0)
        .unwrap();
    for _ in 0..20 {
        assert_eq!(
            balancer
                .select(&endpoints, &ctx(99), None, &mut rng, 0)
                .unwrap()
                .index,
            first.index
        );
    }
    let mut seen = [false; 3];
    for _ in 0..9 {
        let keyless = RequestContext::default();
        seen[balancer
            .select(&endpoints, &keyless, None, &mut rng, 0)
            .unwrap()
            .index] = true;
    }
    assert!(seen.iter().all(|s| *s));
}

#[test]
fn only_the_hashing_algorithm_asks_for_a_key() {
    let endpoints = candidates(&[1]);
    for algorithm in ALGORITHMS
        .into_iter()
        .filter(|a| *a != LbAlgorithm::ConsistentHash)
    {
        assert!(balancer(algorithm, FailurePolicy::FailOpen, &endpoints)
            .hash_key()
            .is_none());
    }
}

#[test]
fn choosing_a_half_open_endpoint_makes_it_the_single_trial() {
    let endpoints = candidates(&[1]);
    eject(&endpoints[0], 100);
    let balancer = balancer(
        LbAlgorithm::RoundRobin,
        FailurePolicy::FailClosed,
        &endpoints,
    );
    let mut rng = Rng::seeded(1);
    assert!(balancer
        .select(&endpoints, &ctx(1), None, &mut rng, 50)
        .is_none());
    assert!(balancer
        .select(&endpoints, &ctx(1), None, &mut rng, 100)
        .is_some());
    assert!(
        balancer
            .select(&endpoints, &ctx(1), None, &mut rng, 100)
            .is_none(),
        "the trial is out; others wait for its verdict"
    );
}

#[test]
fn debug_output_does_not_panic() {
    let endpoints = candidates(&[1]);
    let text = format!(
        "{:?}",
        balancer(LbAlgorithm::RoundRobin, FailurePolicy::FailOpen, &endpoints)
    );
    assert!(text.contains("Balancer"));
}
