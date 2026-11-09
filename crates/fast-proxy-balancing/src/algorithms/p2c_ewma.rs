use crate::{Candidate, Rng};

/// Latency assumed for an endpoint with no samples yet (1 ms), so a fresh
/// endpoint is neither starved nor flooded.
const COLD_START_US: u64 = 1_000;

/// Power of two choices: two random distinct usable endpoints, the one with
/// the lower `ewma * (in_flight + 1) / weight` wins.
pub(crate) fn pick(
    endpoints: &[Candidate],
    rng: &mut Rng,
    usable: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let count = super::count_usable(endpoints.len(), usable);
    match count {
        0 => None,
        1 => super::nth_usable(endpoints.len(), 0, usable),
        _ => {
            let first = rng.below(count);
            let mut second = rng.below(count - 1);
            if second >= first {
                second += 1;
            }
            let a = super::nth_usable(endpoints.len(), first, usable)?;
            let b = super::nth_usable(endpoints.len(), second, usable)?;
            Some(if score_le(&endpoints[a], &endpoints[b]) {
                a
            } else {
                b
            })
        }
    }
}

/// `a`'s score is at most `b`'s, compared by cross-multiplying the weights.
fn score_le(a: &Candidate, b: &Candidate) -> bool {
    let cost = |c: &Candidate| {
        let latency = match c.runtime.ewma_us() {
            0 => COLD_START_US,
            sampled => sampled,
        };
        u128::from(latency) * u128::from(c.runtime.in_flight() + 1)
    };
    cost(a) * u128::from(b.weight) <= cost(b) * u128::from(a.weight)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::candidates;

    fn shares(endpoints: &[Candidate], picks: usize) -> Vec<usize> {
        let mut rng = Rng::seeded(9);
        let mut hits = vec![0; endpoints.len()];
        for _ in 0..picks {
            hits[pick(endpoints, &mut rng, &|_| true).unwrap()] += 1;
        }
        hits
    }

    #[test]
    fn a_consistently_slower_endpoint_gets_a_smaller_share() {
        let endpoints = candidates(&[1, 1, 1]);
        endpoints[0].runtime.record_latency(1_000, 0);
        endpoints[1].runtime.record_latency(1_000, 0);
        endpoints[2].runtime.record_latency(50_000, 0);
        let hits = shares(&endpoints, 3000);
        assert_eq!(hits[2], 0, "the slow one loses every pairing: {hits:?}");
        assert!(hits[0] > 1000 && hits[1] > 1000);
    }

    #[test]
    fn a_fresh_endpoint_is_not_starved() {
        let endpoints = candidates(&[1, 1]);
        endpoints[0].runtime.record_latency(1_000, 0);
        let hits = shares(&endpoints, 1000);
        assert!(hits[1] > 300, "{hits:?}");
    }

    #[test]
    fn in_flight_load_shifts_traffic_to_the_idle_endpoint() {
        let endpoints = candidates(&[1, 1]);
        for e in &endpoints {
            e.runtime.record_latency(1_000, 0);
        }
        let _guards: Vec<_> = (0..4)
            .map(|_| std::sync::Arc::clone(&endpoints[0].runtime).track())
            .collect();
        assert_eq!(shares(&endpoints, 200), [0, 200]);
    }

    #[test]
    fn handles_one_and_none() {
        let endpoints = candidates(&[1, 1, 1]);
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|i| i == 2), Some(2));
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|_| false), None);
    }
}
