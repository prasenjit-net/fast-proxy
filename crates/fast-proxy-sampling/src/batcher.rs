//! Groups samples so the database sees one transaction per batch: a batch goes
//! out when it is full or when its oldest sample has waited long enough. Pure:
//! the clock is a parameter, and nothing here sleeps.

/// Collects items until a batch is due.
#[derive(Debug)]
pub struct Batcher<T> {
    size: usize,
    interval_ms: u64,
    pending: Vec<T>,
    /// When the oldest pending item arrived.
    oldest_ms: Option<u64>,
}

impl<T> Batcher<T> {
    /// A batch is at most `size` items and waits at most `interval_ms`.
    pub fn new(size: usize, interval_ms: u64) -> Self {
        Self {
            size: size.max(1),
            interval_ms,
            pending: Vec::new(),
            oldest_ms: None,
        }
    }

    /// Adds an item; returns the batch if this filled it.
    pub fn push(&mut self, item: T, now_ms: u64) -> Option<Vec<T>> {
        if self.pending.is_empty() {
            self.oldest_ms = Some(now_ms);
        }
        self.pending.push(item);
        if self.pending.len() >= self.size {
            return self.drain();
        }
        None
    }

    /// Returns the pending batch if its oldest item has waited long enough.
    pub fn due(&mut self, now_ms: u64) -> Option<Vec<T>> {
        let deadline = self.deadline_ms()?;
        if now_ms >= deadline {
            return self.drain();
        }
        None
    }

    /// When the pending batch is due, or `None` when nothing is pending.
    pub fn deadline_ms(&self) -> Option<u64> {
        self.oldest_ms
            .map(|oldest| oldest.saturating_add(self.interval_ms))
    }

    /// Takes whatever is pending, if anything.
    pub fn drain(&mut self) -> Option<Vec<T>> {
        self.oldest_ms = None;
        if self.pending.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.pending))
    }

    pub fn len(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_batch_goes_out_at_once_and_in_order() {
        let mut batcher = Batcher::new(3, 250);
        assert_eq!(batcher.push(1, 0), None);
        assert_eq!(batcher.push(2, 1), None);
        assert_eq!(batcher.push(3, 2), Some(vec![1, 2, 3]));
        assert!(batcher.is_empty());
        assert_eq!(batcher.deadline_ms(), None);
    }

    #[test]
    fn a_partial_batch_goes_out_when_its_oldest_item_has_waited_the_interval() {
        let mut batcher = Batcher::new(10, 250);
        batcher.push("a", 1_000);
        batcher.push("b", 1_100);
        assert_eq!(
            batcher.deadline_ms(),
            Some(1_250),
            "counted from the oldest item"
        );
        assert_eq!(batcher.due(1_249), None);
        assert_eq!(batcher.due(1_250), Some(vec!["a", "b"]));
        assert_eq!(batcher.due(9_999), None, "nothing is left");
    }

    #[test]
    fn an_empty_batcher_never_writes_anything() {
        let mut batcher: Batcher<u8> = Batcher::new(5, 100);
        assert_eq!(batcher.due(u64::MAX), None);
        assert_eq!(batcher.drain(), None);
        assert_eq!(batcher.len(), 0);
    }

    #[test]
    fn the_wait_restarts_with_the_next_batch() {
        let mut batcher = Batcher::new(10, 100);
        batcher.push(1, 0);
        assert_eq!(batcher.due(100), Some(vec![1]));
        batcher.push(2, 5_000);
        assert_eq!(batcher.deadline_ms(), Some(5_100));
    }

    #[test]
    fn draining_returns_what_is_pending_for_shutdown() {
        let mut batcher = Batcher::new(10, 100);
        batcher.push(1, 0);
        batcher.push(2, 0);
        assert_eq!(batcher.drain(), Some(vec![1, 2]));
        assert_eq!(batcher.drain(), None);
    }

    #[test]
    fn a_size_of_zero_is_treated_as_one() {
        let mut batcher = Batcher::new(0, 100);
        assert_eq!(batcher.push(7, 0), Some(vec![7]));
    }
}
