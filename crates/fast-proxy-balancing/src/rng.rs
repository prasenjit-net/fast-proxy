/// A tiny xorshift generator for tie-breaks and random choices. Seedable so
/// tests are deterministic; not for anything security related.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator that always produces the same sequence for `seed`.
    pub fn seeded(seed: u64) -> Self {
        Self(seed | 1)
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A value in `0..bound` (`bound` must be at least 1 and below 2^32).
    pub fn below(&mut self, bound: usize) -> usize {
        (((self.next_u64() >> 32) * bound as u64) >> 32) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_sequence() {
        let (mut a, mut b) = (Rng::seeded(7), Rng::seeded(7));
        assert!((0..20).all(|_| a.next_u64() == b.next_u64()));
    }

    #[test]
    fn below_stays_in_range_and_covers_it() {
        let mut rng = Rng::seeded(1);
        let mut seen = [false; 5];
        for _ in 0..200 {
            let value = rng.below(5);
            assert!(value < 5);
            seen[value] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }
}
