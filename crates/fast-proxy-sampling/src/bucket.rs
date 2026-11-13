//! A lock-free token bucket in one `AtomicU64`: the credit in the high half
//! and the low 32 bits of the time of the last refill in the low half. The
//! rate limiter and the storm guard differ only in the unit of credit and the
//! refill rule, so both use this.

use std::sync::atomic::{AtomicU64, Ordering};

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
    // for gaps under 49 days and a longer gap only means a full bucket.
    let now_low = now_ms as u32;
    let mut current = state.load(Ordering::Relaxed);
    loop {
        let (credit, last) = if current == UNSET {
            (u64::from(initial), now_low)
        } else {
            (current >> 32, current as u32)
        };
        let elapsed = u64::from(now_low.wrapping_sub(last));
        let credit = (credit + refill(elapsed)).min(u64::from(cap));
        let taken = credit >= u64::from(cost);
        let remaining = if taken {
            credit - u64::from(cost)
        } else {
            credit
        };
        let next = pack(remaining as u32, now_low);
        match state.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return taken,
            Err(seen) => current = seen,
        }
    }
}
