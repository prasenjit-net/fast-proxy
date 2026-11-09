use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::Snapshot;

/// Shares the current [`Snapshot`] between the data plane (readers) and the
/// config pipeline (the single writer). Reads never block and never see a
/// partially built snapshot.
pub struct SnapshotHolder {
    current: ArcSwap<Snapshot>,
}

impl SnapshotHolder {
    /// Starts with the empty version-0 snapshot.
    pub fn new() -> Self {
        Self {
            current: ArcSwap::from_pointee(Snapshot::empty()),
        }
    }

    /// The snapshot in effect right now. Hold the returned `Arc` for the
    /// whole request so routing stays consistent even across a swap.
    pub fn load(&self) -> Arc<Snapshot> {
        self.current.load_full()
    }

    /// Makes `snapshot` the current one; in-flight holders keep the old one.
    pub fn swap(&self, snapshot: Snapshot) {
        self.current.store(Arc::new(snapshot));
    }
}

impl Default for SnapshotHolder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versioned(version: u64) -> Snapshot {
        Snapshot {
            version,
            ..Snapshot::empty()
        }
    }

    #[test]
    fn starts_empty_at_version_zero() {
        let holder = SnapshotHolder::new();
        assert_eq!(holder.load().version, 0);
        assert!(holder.load().subsystems.is_empty());
    }

    #[test]
    fn swap_publishes_the_new_snapshot_but_old_holders_keep_theirs() {
        let holder = SnapshotHolder::default();
        let before = holder.load();
        holder.swap(versioned(5));
        assert_eq!(before.version, 0);
        assert_eq!(holder.load().version, 5);
    }

    #[test]
    fn readers_on_other_threads_see_only_whole_snapshots() {
        let holder = Arc::new(SnapshotHolder::new());
        let writer = {
            let holder = holder.clone();
            std::thread::spawn(move || {
                for version in 1..=200 {
                    holder.swap(versioned(version));
                }
            })
        };
        let mut last = 0;
        while last < 200 {
            let seen = holder.load().version;
            assert!(seen >= last, "version went backwards: {seen} < {last}");
            last = seen;
        }
        writer.join().unwrap();
    }
}
