use std::cmp::Ordering;

use crate::{Candidate, Rng};

/// The usable endpoint with the lowest `in_flight / weight`; exact ties are
/// broken at random (reservoir sampling, one pass, no allocation).
pub(crate) fn pick(
    endpoints: &[Candidate],
    rng: &mut Rng,
    usable: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut best: Option<usize> = None;
    let mut ties = 0;
    for index in (0..endpoints.len()).filter(|index| usable(*index)) {
        let Some(current) = best else {
            best = Some(index);
            ties = 1;
            continue;
        };
        match compare_load(&endpoints[index], &endpoints[current]) {
            Ordering::Less => {
                best = Some(index);
                ties = 1;
            }
            Ordering::Equal => {
                ties += 1;
                if rng.below(ties) == 0 {
                    best = Some(index);
                }
            }
            Ordering::Greater => {}
        }
    }
    best
}

/// Compares `in_flight / weight` without dividing.
fn compare_load(a: &Candidate, b: &Candidate) -> Ordering {
    let left = u64::from(a.runtime.in_flight()) * u64::from(b.weight);
    let right = u64::from(b.runtime.in_flight()) * u64::from(a.weight);
    left.cmp(&right)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::test_support::candidates;

    fn load(endpoints: &[Candidate], index: usize, count: usize) -> Vec<crate::InFlightGuard> {
        (0..count)
            .map(|_| Arc::clone(&endpoints[index].runtime).track())
            .collect()
    }

    #[test]
    fn picks_the_endpoint_with_the_fewest_in_flight() {
        let endpoints = candidates(&[1, 1, 1]);
        let _a = load(&endpoints, 0, 3);
        let _b = load(&endpoints, 1, 1);
        let _c = load(&endpoints, 2, 2);
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|_| true), Some(1));
    }

    #[test]
    fn weights_scale_the_load() {
        let endpoints = candidates(&[4, 1]);
        let _a = load(&endpoints, 0, 3); // 3/4 = 0.75
        let _b = load(&endpoints, 1, 1); // 1/1 = 1.0
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|_| true), Some(0));
    }

    #[test]
    fn ties_are_broken_randomly_but_only_among_the_tied() {
        let endpoints = candidates(&[1, 1, 1, 1]);
        let _busy = load(&endpoints, 3, 5);
        let mut rng = Rng::seeded(42);
        let mut hits = [0; 4];
        for _ in 0..3000 {
            hits[pick(&endpoints, &mut rng, &|_| true).unwrap()] += 1;
        }
        assert_eq!(hits[3], 0);
        assert!(
            hits[..3].iter().all(|h| (800..1200).contains(h)),
            "{hits:?}"
        );
    }

    #[test]
    fn skips_unusable_endpoints_and_handles_none() {
        let endpoints = candidates(&[1, 1]);
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|i| i == 1), Some(1));
        assert_eq!(pick(&endpoints, &mut Rng::seeded(1), &|_| false), None);
    }
}
