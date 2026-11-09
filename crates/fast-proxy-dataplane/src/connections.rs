use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Counts open client connections (and upgraded tunnels) across all
/// listeners, and lets shutdown wait for them to finish.
#[derive(Debug, Default)]
pub struct ConnectionTracker {
    active: AtomicUsize,
    idle: Notify,
}

impl ConnectionTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Registers one connection; it stays counted until the guard drops.
    pub fn open(self: &Arc<Self>) -> ConnectionGuard {
        self.active.fetch_add(1, Ordering::SeqCst);
        ConnectionGuard {
            tracker: self.clone(),
        }
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Resolves once no connection is open.
    pub async fn wait_idle(&self) {
        loop {
            // Register interest before checking so a wake-up cannot be lost.
            let notified = self.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.active() == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// Keeps one connection counted while alive.
pub struct ConnectionGuard {
    tracker: Arc<ConnectionTracker>,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        if self.tracker.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.tracker.idle.notify_waiters();
        }
    }
}

/// What a request needs to know about the connection it arrived on. The
/// server attaches it to every request so an upgraded tunnel can keep the
/// connection counted and be closed when the drain deadline passes.
#[derive(Clone)]
pub struct ConnectionContext {
    /// Held by a tunnel for its whole life, so draining waits for it.
    pub guard: Arc<ConnectionGuard>,
    /// Cancelled when the drain deadline passes: close everything now.
    pub force_close: CancellationToken,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

    #[tokio::test]
    async fn wait_idle_returns_immediately_when_nothing_is_open() {
        let tracker = ConnectionTracker::new();
        tokio::time::timeout(Duration::from_secs(1), tracker.wait_idle())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wait_idle_waits_for_the_last_guard() {
        let tracker = ConnectionTracker::new();
        let first = tracker.open();
        let second = tracker.open();
        let waiter = {
            let tracker = tracker.clone();
            tokio::spawn(async move { tracker.wait_idle().await })
        };
        drop(first);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "one connection is still open");
        drop(second);
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .unwrap()
            .unwrap();
    }
}
