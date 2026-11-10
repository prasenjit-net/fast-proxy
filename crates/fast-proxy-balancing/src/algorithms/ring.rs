use fast_proxy_model::EndpointId;

use crate::hashing::mix;

/// Virtual nodes per endpoint at average weight.
const VNODES_AT_AVERAGE_WEIGHT: u64 = 160;

/// A consistent-hash ring. Virtual nodes are placed by endpoint **id** (not
/// position), so removing an endpoint only moves the keys it owned.
#[derive(Debug)]
pub(crate) struct Ring {
    /// `(point, endpoint index)` sorted by point.
    points: Vec<(u64, usize)>,
}

impl Ring {
    /// Builds the ring for endpoints given as `(id, weight)`; virtual nodes
    /// are proportional to weight.
    pub(crate) fn build(endpoints: &[(EndpointId, u32)]) -> Self {
        let total: u64 = endpoints.iter().map(|(_, weight)| u64::from(*weight)).sum();
        let mut points = Vec::new();
        for (index, (id, weight)) in endpoints.iter().enumerate() {
            let scaled = VNODES_AT_AVERAGE_WEIGHT * endpoints.len() as u64 * u64::from(*weight);
            let vnodes = (scaled / total.max(1)).max(1);
            let seed = mix(id.0 as u64);
            points.extend((0..vnodes).map(|vnode| (mix(seed ^ mix(vnode)), index)));
        }
        points.sort_unstable();
        Self { points }
    }

    /// The first usable endpoint at or after `key` on the ring.
    pub(crate) fn lookup(&self, key: u64, usable: &impl Fn(usize) -> bool) -> Option<usize> {
        let start = self.points.partition_point(|(point, _)| *point < key);
        (0..self.points.len())
            .map(|step| self.points[(start + step) % self.points.len()].1)
            .find(|index| usable(*index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    fn ring(ids: &[(i64, u32)]) -> Ring {
        let endpoints: Vec<_> = ids.iter().map(|(id, w)| (EndpointId(*id), *w)).collect();
        Ring::build(&endpoints)
    }

    fn keys() -> impl Iterator<Item = u64> {
        let mut rng = Rng::seeded(5);
        std::iter::repeat_with(move || rng.next_u64()).take(20_000)
    }

    #[test]
    fn the_same_key_always_maps_to_the_same_endpoint() {
        let ring = ring(&[(1, 1), (2, 1), (3, 1)]);
        for key in keys().take(500) {
            assert_eq!(ring.lookup(key, &|_| true), ring.lookup(key, &|_| true));
        }
    }

    #[test]
    fn equal_weights_spread_keys_roughly_evenly() {
        let ring = ring(&[(1, 1), (2, 1), (3, 1), (4, 1)]);
        let mut hits = [0; 4];
        for key in keys() {
            hits[ring.lookup(key, &|_| true).unwrap()] += 1;
        }
        assert!(hits.iter().all(|h| (4000..6000).contains(h)), "{hits:?}");
    }

    #[test]
    fn weights_scale_the_share_of_keys() {
        let ring = ring(&[(1, 3), (2, 1)]);
        let mut hits = [0; 2];
        for key in keys() {
            hits[ring.lookup(key, &|_| true).unwrap()] += 1;
        }
        let share = hits[0] as f64 / 20_000.0;
        assert!((0.68..0.82).contains(&share), "{hits:?}");
    }

    #[test]
    fn removing_one_of_n_endpoints_remaps_only_about_one_nth_of_keys() {
        let before = ring(&[(1, 1), (2, 1), (3, 1), (4, 1)]);
        let after = ring(&[(1, 1), (2, 1), (4, 1)]);
        let ids_before = [1, 2, 3, 4];
        let ids_after = [1, 2, 4];
        let moved = keys()
            .filter(|key| {
                let was = ids_before[before.lookup(*key, &|_| true).unwrap()];
                let now = ids_after[after.lookup(*key, &|_| true).unwrap()];
                was != now
            })
            .count();
        let share = moved as f64 / 20_000.0;
        assert!((0.15..0.35).contains(&share), "moved {share}");
    }

    #[test]
    fn unusable_endpoints_are_skipped_and_none_is_reported() {
        let ring = ring(&[(1, 1), (2, 1)]);
        for key in keys().take(200) {
            assert_eq!(ring.lookup(key, &|i| i == 1), Some(1));
            assert_eq!(ring.lookup(key, &|_| false), None);
        }
    }
}
