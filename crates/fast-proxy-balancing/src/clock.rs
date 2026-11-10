use std::sync::OnceLock;
use std::time::Instant;

/// Milliseconds on a process-wide monotonic clock (zero at first use). Every
/// caller of the state machines uses this one epoch so ejection times
/// compare correctly; the state machines themselves only take `now` as input.
pub fn monotonic_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_never_goes_backwards() {
        let first = monotonic_ms();
        assert!(monotonic_ms() >= first);
    }
}
