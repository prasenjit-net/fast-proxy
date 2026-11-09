use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::Candidate;

/// Rotates over the usable endpoints in proportion to their weights: over
/// one full cycle (the sum of the usable weights) each endpoint is picked
/// exactly `weight` times.
pub(crate) fn pick(
    counter: &AtomicU64,
    endpoints: &[Candidate],
    usable: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let total: u64 = (0..endpoints.len())
        .filter(|index| usable(*index))
        .map(|index| u64::from(endpoints[index].weight))
        .sum();
    if total == 0 {
        return None;
    }
    let mut position = counter.fetch_add(1, Relaxed) % total;
    for index in (0..endpoints.len()).filter(|index| usable(*index)) {
        let weight = u64::from(endpoints[index].weight);
        if position < weight {
            return Some(index);
        }
        position -= weight;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::candidates;

    #[test]
    fn a_full_cycle_picks_each_endpoint_exactly_its_weight() {
        let endpoints = candidates(&[5, 1, 3]);
        let counter = AtomicU64::new(0);
        let mut hits = [0; 3];
        for _ in 0..9 {
            hits[pick(&counter, &endpoints, &|_| true).unwrap()] += 1;
        }
        assert_eq!(hits, [5, 1, 3]);
    }

    #[test]
    fn skipping_an_endpoint_renormalizes_over_the_rest() {
        let endpoints = candidates(&[5, 1, 3]);
        let counter = AtomicU64::new(0);
        let mut hits = [0; 3];
        for _ in 0..8 {
            hits[pick(&counter, &endpoints, &|i| i != 1).unwrap()] += 1;
        }
        assert_eq!(hits, [5, 0, 3]);
    }

    #[test]
    fn nothing_usable_picks_nothing() {
        assert_eq!(
            pick(&AtomicU64::new(0), &candidates(&[1, 2]), &|_| false),
            None
        );
    }
}
