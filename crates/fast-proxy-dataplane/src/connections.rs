use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Counts open client connections across all listeners.
#[derive(Debug, Default)]
pub struct ConnectionTracker {
    active: AtomicUsize,
}

impl ConnectionTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers one connection; it stays counted until the guard drops.
    pub fn open(self: &Arc<Self>) -> ConnectionGuard {
        self.active.fetch_add(1, Ordering::Relaxed);
        ConnectionGuard {
            tracker: self.clone(),
        }
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }
}

/// Keeps one connection counted while alive.
pub struct ConnectionGuard {
    tracker: Arc<ConnectionTracker>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.tracker.active.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_open_connections_until_their_guards_drop() {
        let tracker = ConnectionTracker::new();
        assert_eq!(tracker.active(), 0);
        let first = tracker.open();
        let second = tracker.open();
        assert_eq!(tracker.active(), 2);
        drop(first);
        assert_eq!(tracker.active(), 1);
        drop(second);
        assert_eq!(tracker.active(), 0);
    }
}
