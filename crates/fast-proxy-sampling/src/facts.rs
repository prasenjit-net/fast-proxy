//! What the data plane tells the sampler about one request: the facts known
//! before forwarding ([`RequestHead`]) and after the response ([`Outcome`]).
//! Plain data, no HTTP types, so the decision stays pure.

use std::net::SocketAddr;

/// The most upstream attempts one request can make (the retry policy allows 5).
pub const MAX_ATTEMPTS: usize = 5;

/// How a connectivity failure showed itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    ConnectRefused,
    ConnectTimeout,
    TlsHandshake,
    /// The connection was reset or closed before a response head.
    ConnectionReset,
    /// No response head within the response timeout.
    ResponseTimeout,
}

impl FailureKind {
    /// The API spelling, also used in the storm guard's key.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectRefused => "connect_refused",
            Self::ConnectTimeout => "connect_timeout",
            Self::TlsHandshake => "tls_handshake",
            Self::ConnectionReset => "connection_reset",
            Self::ResponseTimeout => "response_timeout",
        }
    }
}

/// One upstream attempt. An upstream's own answer (any status, 5xx included)
/// is `failure: None`: it is the application talking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attempt {
    /// The endpoint's position in its group; the address is looked up in the
    /// snapshot only when a sample is kept.
    pub endpoint_index: u16,
    pub failure: Option<FailureKind>,
    pub upstream_status: Option<u16>,
}

/// The attempts of one request, kept inline so recording them allocates
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttemptLog {
    attempts: [Option<Attempt>; MAX_ATTEMPTS],
    len: u8,
}

impl Default for AttemptLog {
    fn default() -> Self {
        Self {
            attempts: [None; MAX_ATTEMPTS],
            len: 0,
        }
    }
}

impl AttemptLog {
    /// Records an attempt; one beyond [`MAX_ATTEMPTS`] is ignored.
    pub fn push(&mut self, attempt: Attempt) {
        if usize::from(self.len) < MAX_ATTEMPTS {
            self.attempts[usize::from(self.len)] = Some(attempt);
            self.len += 1;
        }
    }

    pub fn len(&self) -> usize {
        usize::from(self.len)
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &Attempt> {
        self.attempts.iter().take(self.len()).flatten()
    }

    /// How many attempts ended in a connectivity failure.
    pub fn connectivity_failures(&self) -> usize {
        self.iter()
            .filter(|attempt| attempt.failure.is_some())
            .count()
    }
}

/// Who produced the response the client got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// An upstream answered (any status).
    Upstream,
    /// The proxy answered 502 or 504 because the last attempt failed.
    ProxyFailure,
    /// A maintenance scope answered; no upstream was contacted.
    Maintenance,
    /// Any other answer the proxy made itself: the unmatched-host 404, a
    /// redirect, a disabled subsystem.
    Fixed,
}

/// Facts about the finished request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: u16,
    pub elapsed_ms: u32,
    pub answer: Answer,
    pub attempts: AttemptLog,
    pub bytes_in: u64,
    pub bytes_out: u64,
    /// The client left before the response finished (reported as 499).
    pub client_cancelled: bool,
}

/// Why a request's headers look unusual (decision J7). A bit set, so noting
/// one costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UnusualFlags(u8);

impl UnusualFlags {
    pub const TOO_MANY_HEADERS: Self = Self(1);
    pub const HUGE_HEADER_VALUE: Self = Self(1 << 1);
    pub const NON_VISIBLE_VALUE: Self = Self(1 << 2);
    pub const DUPLICATE_SINGLETON: Self = Self(1 << 3);
    pub const LENGTH_WITH_TRANSFER_ENCODING: Self = Self(1 << 4);
    pub const UNKNOWN_METHOD: Self = Self(1 << 5);
    pub const HUGE_PATH: Self = Self(1 << 6);

    const NAMES: [(Self, &'static str); 7] = [
        (Self::TOO_MANY_HEADERS, "too_many_headers"),
        (Self::HUGE_HEADER_VALUE, "huge_header_value"),
        (Self::NON_VISIBLE_VALUE, "non_visible_value"),
        (Self::DUPLICATE_SINGLETON, "duplicate_singleton_header"),
        (
            Self::LENGTH_WITH_TRANSFER_ENCODING,
            "length_with_transfer_encoding",
        ),
        (Self::UNKNOWN_METHOD, "unknown_method"),
        (Self::HUGE_PATH, "huge_path"),
    ];

    pub fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The names of the flags set, for the sample.
    pub fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .filter(|(flag, _)| self.contains(*flag))
            .map(|(_, name)| *name)
            .collect()
    }
}

/// What was known of the request before it was forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestHead {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    /// The allow-listed request headers, as received (redacted later).
    pub headers: Vec<(String, String)>,
    pub flags: UnusualFlags,
    pub client: SocketAddr,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(failure: Option<FailureKind>) -> Attempt {
        Attempt {
            endpoint_index: 0,
            failure,
            upstream_status: None,
        }
    }

    #[test]
    fn the_attempt_log_holds_five_and_ignores_the_rest() {
        let mut log = AttemptLog::default();
        assert!(log.is_empty());
        for _ in 0..8 {
            log.push(attempt(None));
        }
        assert_eq!(log.len(), MAX_ATTEMPTS);
        assert_eq!(log.iter().count(), MAX_ATTEMPTS);
    }

    #[test]
    fn only_connectivity_failures_count_as_failures() {
        let mut log = AttemptLog::default();
        log.push(attempt(Some(FailureKind::ConnectRefused)));
        log.push(Attempt {
            upstream_status: Some(503),
            ..attempt(None)
        });
        log.push(attempt(Some(FailureKind::ResponseTimeout)));
        assert_eq!(log.connectivity_failures(), 2);
    }

    #[test]
    fn unusual_flags_combine_and_name_themselves() {
        let flags = UnusualFlags::default()
            .with(UnusualFlags::HUGE_PATH)
            .with(UnusualFlags::UNKNOWN_METHOD);
        assert!(flags.contains(UnusualFlags::HUGE_PATH));
        assert!(!flags.contains(UnusualFlags::TOO_MANY_HEADERS));
        assert_eq!(flags.names(), ["unknown_method", "huge_path"]);
        assert!(UnusualFlags::default().is_empty());
        assert!(!flags.is_empty());
    }

    #[test]
    fn failure_names_are_stable() {
        assert_eq!(FailureKind::ConnectRefused.as_str(), "connect_refused");
        assert_eq!(FailureKind::TlsHandshake.as_str(), "tls_handshake");
    }
}
