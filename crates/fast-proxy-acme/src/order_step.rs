//! The steps of one ACME order (section 4.1 of the M4 plan) as a pure state
//! machine. The runner performs each step's I/O and feeds the outcome back as
//! an [`OrderEvent`]; the machine says what comes next, whether challenges
//! must be cleared, and how the order ended for the metrics.

use std::fmt;

use crate::FailureKind;

/// Where an order stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStep {
    /// Loading the account and creating the order at the CA.
    Starting,
    /// The order exists; its challenges are not yet published.
    OrderCreated,
    /// The challenges are in the database (and so in every snapshot, soon).
    Published,
    /// Other instances had time to see the challenges.
    Propagated,
    /// Every challenge URL answered with its key authorization.
    SelfChecked,
    /// The CA was told the challenges are ready and is checking them.
    Validating,
    /// The CA validated every name; the CSR was sent.
    Finalizing,
    /// The chain was downloaded and passed the checks.
    Issued,
    /// The certificate is stored and attached: the order succeeded.
    Stored,
    /// The order failed after `after`; nothing was stored.
    Failed {
        after: &'static str,
        kind: FailureKind,
        /// Challenges were published, so they must be cleared.
        published: bool,
    },
}

/// The outcome of the I/O a step performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderEvent {
    OrderCreated,
    ChallengesPublished,
    PropagationElapsed,
    SelfCheckPassed,
    ValidationRequested,
    Validated,
    CertificateAccepted,
    CertificateStored,
    Failed(FailureKind),
}

/// An event that does not fit the current step: a bug in the runner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub step: &'static str,
    pub event: OrderEvent,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "order event {:?} does not fit step {}",
            self.event, self.step
        )
    }
}

impl std::error::Error for TransitionError {}

impl OrderStep {
    /// The step after `event`. A failure is accepted at any step that has not
    /// ended; anything else must be the one event the step waits for.
    pub fn next(self, event: OrderEvent) -> Result<Self, TransitionError> {
        if self.is_finished() {
            return Err(self.reject(event));
        }
        if let OrderEvent::Failed(kind) = event {
            return Ok(Self::Failed {
                after: self.name(),
                kind,
                published: self.has_published(),
            });
        }
        let next = match (self, event) {
            (Self::Starting, OrderEvent::OrderCreated) => Self::OrderCreated,
            (Self::OrderCreated, OrderEvent::ChallengesPublished) => Self::Published,
            (Self::Published, OrderEvent::PropagationElapsed) => Self::Propagated,
            (Self::Propagated, OrderEvent::SelfCheckPassed) => Self::SelfChecked,
            (Self::SelfChecked, OrderEvent::ValidationRequested) => Self::Validating,
            (Self::Validating, OrderEvent::Validated) => Self::Finalizing,
            (Self::Finalizing, OrderEvent::CertificateAccepted) => Self::Issued,
            (Self::Issued, OrderEvent::CertificateStored) => Self::Stored,
            _ => return Err(self.reject(event)),
        };
        Ok(next)
    }

    /// Whether the order ended, either way.
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Stored | Self::Failed { .. })
    }

    /// Whether challenges of this order may be in the database and must be
    /// cleared when it fails (storing the certificate clears them itself).
    pub fn must_clear_challenges(self) -> bool {
        match self {
            Self::Failed { published, .. } => published,
            other => other.has_published(),
        }
    }

    /// The outcome label for metrics and logs, once finished: `issued`,
    /// `rate_limited`, `self_check_failed` or `failed`.
    pub fn outcome(self) -> Option<&'static str> {
        match self {
            Self::Stored => Some("issued"),
            Self::Failed {
                kind: FailureKind::RateLimited { .. },
                ..
            } => Some("rate_limited"),
            Self::Failed {
                after: "propagated",
                ..
            } => Some("self_check_failed"),
            Self::Failed { .. } => Some("failed"),
            _ => None,
        }
    }

    /// The step's name, as status and logs show it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::OrderCreated => "order_created",
            Self::Published => "published",
            Self::Propagated => "propagated",
            Self::SelfChecked => "self_checked",
            Self::Validating => "validating",
            Self::Finalizing => "finalizing",
            Self::Issued => "issued",
            Self::Stored => "stored",
            Self::Failed { .. } => "failed",
        }
    }

    fn has_published(self) -> bool {
        matches!(
            self,
            Self::Published
                | Self::Propagated
                | Self::SelfChecked
                | Self::Validating
                | Self::Finalizing
                | Self::Issued
        )
    }

    fn reject(self, event: OrderEvent) -> TransitionError {
        TransitionError {
            step: self.name(),
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAPPY_PATH: [OrderEvent; 8] = [
        OrderEvent::OrderCreated,
        OrderEvent::ChallengesPublished,
        OrderEvent::PropagationElapsed,
        OrderEvent::SelfCheckPassed,
        OrderEvent::ValidationRequested,
        OrderEvent::Validated,
        OrderEvent::CertificateAccepted,
        OrderEvent::CertificateStored,
    ];

    fn after(events: &[OrderEvent]) -> OrderStep {
        events
            .iter()
            .try_fold(OrderStep::Starting, |step, event| step.next(*event))
            .unwrap()
    }

    #[test]
    fn the_happy_path_ends_stored_and_issued() {
        let end = after(&HAPPY_PATH);
        assert_eq!(end, OrderStep::Stored);
        assert!(end.is_finished());
        assert!(!end.must_clear_challenges(), "storing clears them itself");
        assert_eq!(end.outcome(), Some("issued"));
    }

    #[test]
    fn an_out_of_order_event_is_refused() {
        let err = OrderStep::Starting.next(OrderEvent::Validated).unwrap_err();
        assert_eq!(err.step, "starting");
        assert!(err.to_string().contains("Validated"));
        assert!(OrderStep::Stored.next(OrderEvent::OrderCreated).is_err());
        let failed = OrderStep::Starting
            .next(OrderEvent::Failed(FailureKind::Transient))
            .unwrap();
        assert!(failed
            .next(OrderEvent::Failed(FailureKind::Transient))
            .is_err());
    }

    #[test]
    fn a_failure_before_publishing_needs_no_cleanup_and_after_it_does() {
        for published in 0..HAPPY_PATH.len() - 1 {
            let step = after(&HAPPY_PATH[..published]);
            let failed = step
                .next(OrderEvent::Failed(FailureKind::Transient))
                .unwrap();
            assert_eq!(
                failed.must_clear_challenges(),
                published >= 2,
                "failed after {}",
                step.name()
            );
            assert!(failed.is_finished());
            assert_eq!(step.outcome(), None, "not finished yet");
        }
    }

    #[test]
    fn outcomes_tell_rate_limits_and_self_check_failures_apart() {
        let rate_limited = OrderStep::Validating
            .next(OrderEvent::Failed(FailureKind::RateLimited {
                retry_at_ms: None,
            }))
            .unwrap();
        assert_eq!(rate_limited.outcome(), Some("rate_limited"));
        let self_check = OrderStep::Propagated
            .next(OrderEvent::Failed(FailureKind::Transient))
            .unwrap();
        assert_eq!(self_check.outcome(), Some("self_check_failed"));
        let other = OrderStep::Finalizing
            .next(OrderEvent::Failed(FailureKind::Permanent))
            .unwrap();
        assert_eq!(other.outcome(), Some("failed"));
        assert_eq!(other.name(), "failed");
    }
}
