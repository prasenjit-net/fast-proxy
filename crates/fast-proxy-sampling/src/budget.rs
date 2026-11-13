//! The process-wide limit on memory held by teed bodies (decision J9). A
//! request that cannot reserve its caps is simply not captured; nothing blocks
//! and nothing waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Counts the bytes reserved by teed bodies.
#[derive(Debug)]
pub struct BodyBudget {
    limit: u64,
    used: AtomicU64,
}

impl BodyBudget {
    pub fn new(limit_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            limit: limit_bytes,
            used: AtomicU64::new(0),
        })
    }

    /// Reserves `bytes` if they fit; the reservation is returned when the guard
    /// is dropped, however the request ends.
    pub fn reserve(self: &Arc<Self>, bytes: u64) -> Option<BodyReservation> {
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let next = used.checked_add(bytes).filter(|next| *next <= self.limit)?;
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => {
                    return Some(BodyReservation {
                        budget: self.clone(),
                        bytes,
                    })
                }
                Err(seen) => used = seen,
            }
        }
    }

    /// Bytes reserved right now.
    pub fn used(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// Bytes held from a [`BodyBudget`]; released on drop.
#[derive(Debug)]
pub struct BodyReservation {
    budget: Arc<BodyBudget>,
    bytes: u64,
}

impl BodyReservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl Drop for BodyReservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reservation_that_fits_is_granted_and_released_on_drop() {
        let budget = BodyBudget::new(100);
        let first = budget.reserve(60).unwrap();
        assert_eq!(budget.used(), 60);
        assert!(budget.reserve(41).is_none(), "60 + 41 is over the limit");
        let second = budget.reserve(40).unwrap();
        assert_eq!(budget.used(), 100);
        drop(first);
        assert_eq!(budget.used(), 40);
        drop(second);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn a_zero_budget_grants_nothing_and_a_zero_reservation_always_fits() {
        let budget = BodyBudget::new(0);
        assert!(budget.reserve(1).is_none());
        assert!(budget.reserve(0).is_some());
    }

    #[test]
    fn an_absurd_request_does_not_overflow() {
        let budget = BodyBudget::new(u64::MAX);
        let held = budget.reserve(u64::MAX - 1).unwrap();
        assert!(budget.reserve(u64::MAX).is_none());
        drop(held);
        assert_eq!(budget.used(), 0);
    }

    #[test]
    fn concurrent_reservations_never_exceed_the_limit() {
        let budget = BodyBudget::new(1000);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let budget = budget.clone();
                std::thread::spawn(move || {
                    let mut held = Vec::new();
                    for _ in 0..500 {
                        if let Some(reservation) = budget.reserve(10) {
                            assert!(budget.used() <= 1000);
                            held.push(reservation);
                        }
                    }
                    held
                })
            })
            .collect();
        // The reservations stay alive until every thread is done.
        let held: Vec<Vec<BodyReservation>> =
            threads.into_iter().map(|t| t.join().unwrap()).collect();
        let granted: usize = held.iter().map(Vec::len).sum();
        assert_eq!(
            granted, 100,
            "exactly limit / size were granted while all were held"
        );
        assert_eq!(budget.used(), 1000);
        drop(held);
        assert_eq!(budget.used(), 0, "and every one was released");
    }
}
