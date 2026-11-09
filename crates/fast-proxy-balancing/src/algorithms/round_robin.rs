use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use super::{count_usable, nth_usable};

/// The next usable endpoint in order. The rotation is over the usable ones
/// only, so skipping an ineligible endpoint stays fair among the rest.
pub(crate) fn pick(
    counter: &AtomicU64,
    len: usize,
    usable: &impl Fn(usize) -> bool,
) -> Option<usize> {
    let count = count_usable(len, usable);
    if count == 0 {
        return None;
    }
    let nth = (counter.fetch_add(1, Relaxed) % count as u64) as usize;
    nth_usable(len, nth, usable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_through_every_endpoint_evenly() {
        let counter = AtomicU64::new(0);
        let mut hits = [0; 3];
        for _ in 0..300 {
            hits[pick(&counter, 3, &|_| true).unwrap()] += 1;
        }
        assert_eq!(hits, [100, 100, 100]);
    }

    #[test]
    fn skipping_an_unusable_endpoint_stays_fair_among_the_rest() {
        let counter = AtomicU64::new(0);
        let mut hits = [0; 4];
        for _ in 0..300 {
            hits[pick(&counter, 4, &|i| i != 1).unwrap()] += 1;
        }
        assert_eq!(hits, [100, 0, 100, 100]);
    }

    #[test]
    fn nothing_usable_picks_nothing() {
        assert_eq!(pick(&AtomicU64::new(0), 3, &|_| false), None);
        assert_eq!(pick(&AtomicU64::new(0), 0, &|_| true), None);
    }
}
