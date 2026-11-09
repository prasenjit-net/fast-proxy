//! The selection algorithms. Each takes a `usable` predicate (the caller's
//! idea of eligibility) and returns an index, or `None` when nothing is usable.

pub(crate) mod least_connections;
pub(crate) mod p2c_ewma;
pub(crate) mod ring;
pub(crate) mod round_robin;
pub(crate) mod weighted_round_robin;

/// The index of the `nth` (zero-based) usable endpoint.
pub(crate) fn nth_usable(len: usize, nth: usize, usable: &impl Fn(usize) -> bool) -> Option<usize> {
    (0..len).filter(|index| usable(*index)).nth(nth)
}

/// How many endpoints are usable.
pub(crate) fn count_usable(len: usize, usable: &impl Fn(usize) -> bool) -> usize {
    (0..len).filter(|index| usable(*index)).count()
}
