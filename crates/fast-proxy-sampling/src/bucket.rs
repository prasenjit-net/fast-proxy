//! A lock-free token bucket in one `AtomicU64`: the credit in the high half
//! and the low 32 bits of the time of the last refill in the low half. The
//! rate limiter and the storm guard differ only in the unit of credit and the
//! refill rule, so both use this.

use std::sync::atomic::{AtomicU64, Ordering};

/// How far behind the stored time a caller's clock reading may be and still be a
/// late arrival rather than a gap of weeks. Callers read the clock before they
/// reach the bucket, so under load a reading a few milliseconds older than the
/// stored one is normal.
const LATE_ARRIVAL_MS: u32 = 60_000;

/// The state of a bucket nobody has used yet; the first call fills it.
pub(crate) const UNSET: u64 = u64::MAX;

fn pack(credit: u32, time_low: u32) -> u64 {
    (u64::from(credit) << 32) | u64::from(time_low)
}

/// Refills the bucket for the time that passed (`refill` maps elapsed
/// milliseconds to credit), then takes `cost` if there is enough. The refill is
/// stored even when the take fails, so the stored time keeps up with the clock.
/// `cap` is the most credit the bucket holds and `initial` what an unused
/// bucket starts with.
pub(crate) fn take(
    state: &AtomicU64,
    now_ms: u64,
    (cap, initial): (u32, u32),
    refill: impl Fn(u64) -> u64,
    cost: u32,
) -> bool {
    // Only the low 32 bits of the time are kept; a wrapping difference is exact
    // for gaps under 49 days and a longer gap only means a full bucket. A reading
    // slightly older than the stored one (another thread got there first) must
    // not look like a gap of 49 days and refill the bucket: it adds no credit and
    // leaves the stored time alone.
    let now_low = now_ms as u32;
    let mut current = state.load(Ordering::Relaxed);
    loop {
        let (credit, last) = if current == UNSET {
            (u64::from(initial), now_low)
        } else {
            (current >> 32, current as u32)
        };
        let behind = now_low.wrapping_sub(last);
        let late = behind > u32::MAX - LATE_ARRIVAL_MS;
        let elapsed = if late { 0 } else { u64::from(behind) };
        let stamp = if late { last } else { now_low };
        let credit = (credit + refill(elapsed)).min(u64::from(cap));
        let taken = credit >= u64::from(cost);
        let remaining = if taken {
            credit - u64::from(cost)
        } else {
            credit
        };
        let next = pack(remaining as u32, stamp);
        match state.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return taken,
            Err(seen) => current = seen,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_per_second(state: &AtomicU64, now_ms: u64) -> bool {
        take(state, now_ms, (1000, 1000), |elapsed| elapsed, 1000)
    }

    #[test]
    fn a_late_clock_reading_adds_no_credit() {
        let state = AtomicU64::new(UNSET);
        assert!(
            one_per_second(&state, 10_000),
            "the first take uses the initial credit"
        );
        assert!(!one_per_second(&state, 10_005), "an empty bucket refuses");
        // Another thread read the clock 2 ms earlier than the one above did.
        assert!(
            !one_per_second(&state, 10_003),
            "a late reading must not refill the bucket"
        );
        assert!(!one_per_second(&state, 10_006));
    }

    #[test]
    fn a_late_reading_does_not_move_the_stored_time_back() {
        let state = AtomicU64::new(UNSET);
        assert!(one_per_second(&state, 10_000));
        assert!(!one_per_second(&state, 10_900));
        assert!(!one_per_second(&state, 10_100), "late");
        // 1000 ms after the first take, not after the late reading.
        assert!(one_per_second(&state, 11_000));
    }

    #[test]
    fn credit_returns_with_time() {
        let state = AtomicU64::new(UNSET);
        assert!(one_per_second(&state, 0));
        assert!(!one_per_second(&state, 500));
        assert!(one_per_second(&state, 1_000));
    }
}
