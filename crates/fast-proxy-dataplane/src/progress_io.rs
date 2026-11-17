//! Watching a client connection for stalls hyper does not bound (spike G2,
//! ADR 0029): a connection that never sends a byte, writes to a client that
//! stopped reading, and a tunnel that carries nothing.
//!
//! [`ProgressIo`] wraps the stream and records progress in atomics; [`watch`]
//! checks them on a coarse tick and returns which stall it found, so the
//! connection's task can close it. Nothing is done per byte beyond a few
//! relaxed stores.

use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use fast_proxy_balancing::monotonic_ms;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Why a connection was closed for lack of progress; a fixed set, so it can be
/// a metric label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stall {
    /// Not one byte arrived within the header read timeout.
    FirstByte,
    /// Writes to the client stayed blocked past the send timeout.
    Send,
    /// A tunnel carried nothing either way past its idle timeout.
    TunnelIdle,
}

impl Stall {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FirstByte => "first_byte",
            Self::Send => "send",
            Self::TunnelIdle => "tunnel_idle",
        }
    }
}

/// What a stream did and when, in [`monotonic_ms`]. A time of 0 means "never";
/// times are stored plus one so a reading of 0 at start-up is not lost.
#[derive(Debug)]
pub(crate) struct Progress {
    opened: u64,
    bytes_read: AtomicU64,
    last_activity: AtomicU64,
    write_blocked_since: AtomicU64,
}

impl Progress {
    fn new(now: u64) -> Self {
        Self {
            opened: now,
            bytes_read: AtomicU64::new(0),
            last_activity: AtomicU64::new(now + 1),
            write_blocked_since: AtomicU64::new(0),
        }
    }

    fn read(&self, bytes: usize, now: u64) {
        self.bytes_read.fetch_add(bytes as u64, Relaxed);
        self.last_activity.store(now + 1, Relaxed);
    }

    fn wrote(&self, now: u64) {
        self.write_blocked_since.store(0, Relaxed);
        self.last_activity.store(now + 1, Relaxed);
    }

    fn write_blocked(&self, now: u64) {
        // Keep the first moment it blocked; later attempts do not move it.
        let _ = self
            .write_blocked_since
            .compare_exchange(0, now + 1, Relaxed, Relaxed);
    }
}

/// The rules a watched connection is held to; `None` turns a rule off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Deadlines {
    pub first_byte: Option<Duration>,
    pub send: Option<Duration>,
    pub idle: Option<Duration>,
}

impl Deadlines {
    /// The stall `progress` shows at `now`, if any.
    fn check(&self, progress: &Progress, now: u64) -> Option<Stall> {
        let past = |since: u64, limit: Duration| now.saturating_sub(since) > millis(limit);
        if let Some(limit) = self.first_byte {
            if progress.bytes_read.load(Relaxed) == 0 && past(progress.opened, limit) {
                return Some(Stall::FirstByte);
            }
        }
        if let Some(limit) = self.send {
            let blocked = progress.write_blocked_since.load(Relaxed);
            if blocked != 0 && past(blocked - 1, limit) {
                return Some(Stall::Send);
            }
        }
        if let Some(limit) = self.idle {
            if past(progress.last_activity.load(Relaxed) - 1, limit) {
                return Some(Stall::TunnelIdle);
            }
        }
        None
    }

    /// How often to check: a quarter of the shortest rule, between 50 ms and 1 s.
    fn tick(&self) -> Duration {
        [self.first_byte, self.send, self.idle]
            .into_iter()
            .flatten()
            .min()
            .map_or(Duration::from_secs(1), |shortest| shortest / 4)
            .clamp(Duration::from_millis(50), Duration::from_secs(1))
    }

    fn is_off(&self) -> bool {
        self.first_byte.is_none() && self.send.is_none() && self.idle.is_none()
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Resolves with the first stall `progress` shows under `deadlines`; never, when
/// every rule is off. The caller closes the connection when it resolves.
pub(crate) async fn watch(progress: Arc<Progress>, deadlines: Deadlines) -> Stall {
    if deadlines.is_off() {
        return std::future::pending().await;
    }
    let tick = deadlines.tick();
    loop {
        tokio::time::sleep(tick).await;
        if let Some(stall) = deadlines.check(&progress, monotonic_ms()) {
            return stall;
        }
    }
}

/// A stream that records its progress for [`watch`].
pub(crate) struct ProgressIo<S> {
    inner: S,
    progress: Arc<Progress>,
}

impl<S> ProgressIo<S> {
    pub(crate) fn new(inner: S) -> Self {
        Self {
            inner,
            progress: Arc::new(Progress::new(monotonic_ms())),
        }
    }

    /// Wraps `inner` recording into `progress`, which another stream may share
    /// (both sides of a tunnel count as one).
    pub(crate) fn sharing(inner: S, progress: Arc<Progress>) -> Self {
        Self { inner, progress }
    }

    pub(crate) fn progress(&self) -> Arc<Progress> {
        self.progress.clone()
    }
}

/// A fresh progress record for streams wrapped with [`ProgressIo::sharing`].
pub(crate) fn new_progress() -> Arc<Progress> {
    Arc::new(Progress::new(monotonic_ms()))
}

impl<S: AsyncRead + Unpin> AsyncRead for ProgressIo<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let polled = Pin::new(&mut self.inner).poll_read(cx, buf);
        let read = buf.filled().len() - before;
        if read > 0 {
            self.progress.read(read, monotonic_ms());
        }
        polled
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ProgressIo<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write(cx, buf);
        self.note_write(&polled);
        polled
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        let polled = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        self.note_write(&polled);
        polled
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let polled = Pin::new(&mut self.inner).poll_flush(cx);
        if polled.is_pending() {
            self.progress.write_blocked(monotonic_ms());
        }
        polled
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<S> ProgressIo<S> {
    fn note_write(&self, polled: &Poll<std::io::Result<usize>>) {
        match polled {
            Poll::Ready(Ok(written)) if *written > 0 => self.progress.wrote(monotonic_ms()),
            Poll::Pending => self.progress.write_blocked(monotonic_ms()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    fn deadlines(first_byte: Option<u64>, send: Option<u64>, idle: Option<u64>) -> Deadlines {
        let secs = |s: Option<u64>| s.map(Duration::from_secs);
        Deadlines {
            first_byte: secs(first_byte),
            send: secs(send),
            idle: secs(idle),
        }
    }

    #[test]
    fn a_connection_without_a_first_byte_stalls_after_the_deadline() {
        let progress = Progress::new(1_000);
        let rules = deadlines(Some(10), None, None);
        assert_eq!(rules.check(&progress, 11_000), None, "exactly the deadline");
        assert_eq!(rules.check(&progress, 11_001), Some(Stall::FirstByte));
        progress.read(1, 2_000);
        assert_eq!(rules.check(&progress, 60_000), None, "one byte is enough");
    }

    #[test]
    fn blocked_writes_stall_only_while_they_stay_blocked() {
        let progress = Progress::new(0);
        let rules = deadlines(None, Some(5), None);
        progress.write_blocked(1_000);
        progress.write_blocked(3_000);
        assert_eq!(rules.check(&progress, 6_000), None);
        assert_eq!(
            rules.check(&progress, 6_001),
            Some(Stall::Send),
            "counted from the first block"
        );
        progress.wrote(6_001);
        assert_eq!(rules.check(&progress, 60_000), None, "progress clears it");
    }

    #[test]
    fn a_tunnel_stalls_when_nothing_moves_either_way() {
        let progress = Progress::new(0);
        let rules = deadlines(None, None, Some(60));
        progress.read(10, 30_000);
        assert_eq!(rules.check(&progress, 90_000), None);
        progress.wrote(85_000);
        assert_eq!(rules.check(&progress, 145_000), None);
        assert_eq!(rules.check(&progress, 145_001), Some(Stall::TunnelIdle));
    }

    #[test]
    fn a_reading_at_time_zero_still_counts() {
        let progress = Progress::new(0);
        progress.write_blocked(0);
        let rules = deadlines(None, Some(1), None);
        assert_eq!(rules.check(&progress, 1_001), Some(Stall::Send));
    }

    #[test]
    fn the_tick_follows_the_shortest_rule_within_bounds() {
        assert_eq!(deadlines(Some(10), Some(60), None).tick(), SECOND);
        assert_eq!(
            Deadlines {
                first_byte: Some(Duration::from_millis(400)),
                send: None,
                idle: None,
            }
            .tick(),
            Duration::from_millis(100)
        );
        assert_eq!(
            Deadlines {
                first_byte: Some(Duration::from_millis(40)),
                send: None,
                idle: None,
            }
            .tick(),
            Duration::from_millis(50)
        );
    }

    #[tokio::test]
    async fn watching_with_every_rule_off_never_resolves() {
        let progress = Arc::new(Progress::new(monotonic_ms()));
        let off = deadlines(None, None, None);
        let watched = tokio::time::timeout(Duration::from_millis(100), watch(progress, off)).await;
        assert!(watched.is_err());
    }

    #[tokio::test]
    async fn the_wrapper_records_reads_and_blocked_writes() {
        let (client, server) = tokio::io::duplex(8);
        let mut wrapped = ProgressIo::new(server);
        let progress = wrapped.progress();
        let mut client = client;
        tokio::io::AsyncWriteExt::write_all(&mut client, b"abc")
            .await
            .unwrap();
        let mut buf = [0u8; 8];
        let read = tokio::io::AsyncReadExt::read(&mut wrapped, &mut buf)
            .await
            .unwrap();
        assert_eq!(read, 3);
        assert_eq!(progress.bytes_read.load(Relaxed), 3);
        // The duplex holds 8 bytes; nobody reads the client side, so a larger
        // write blocks.
        let write = tokio::io::AsyncWriteExt::write_all(&mut wrapped, &[0u8; 64]);
        assert!(tokio::time::timeout(Duration::from_millis(50), write)
            .await
            .is_err());
        assert_ne!(progress.write_blocked_since.load(Relaxed), 0);
    }
}
