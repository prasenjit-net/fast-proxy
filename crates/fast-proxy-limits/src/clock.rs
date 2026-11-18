use std::sync::OnceLock;
use std::time::Instant;

/// Nanoseconds since the first call in this process: a monotonic clock fine
/// enough for rates of thousands per second, and an `u64` lasts centuries.
pub fn monotonic_ns() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed();
    u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_never_goes_backwards() {
        let first = monotonic_ns();
        let second = monotonic_ns();
        assert!(second >= first);
    }
}
