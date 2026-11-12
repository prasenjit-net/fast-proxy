use crate::FieldError;

/// Why a request was sampled. The set is fixed, so the names are safe metric
/// labels, and the bit positions are stored in the database: never reorder.
/// The declaration order is the priority: the first trigger that matched is
/// the sample's primary one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Trigger {
    /// A connectivity failure ended the request (the proxy's own 502 or 504).
    UpstreamFailure,
    /// A connectivity failure was retried and the request then got an answer.
    Retried,
    /// Slower than a fixed threshold.
    Slow,
    /// Slower than a multiple of the route's rolling p99.
    SlowP99,
    /// One client address produced many 4xx responses.
    ClientErrors,
    /// A request body larger than the threshold.
    OversizedRequest,
    /// A response body larger than the threshold.
    OversizedResponse,
    /// Malformed or unusual request headers.
    UnusualHeaders,
    /// The small random sample kept for comparison.
    Baseline,
}

impl Trigger {
    /// Every trigger, in priority order.
    pub const ALL: [Trigger; 9] = [
        Trigger::UpstreamFailure,
        Trigger::Retried,
        Trigger::Slow,
        Trigger::SlowP99,
        Trigger::ClientErrors,
        Trigger::OversizedRequest,
        Trigger::OversizedResponse,
        Trigger::UnusualHeaders,
        Trigger::Baseline,
    ];

    /// The stored and API spelling, also the metric label.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamFailure => "upstream_failure",
            Self::Retried => "retried",
            Self::Slow => "slow",
            Self::SlowP99 => "slow_p99",
            Self::ClientErrors => "client_errors",
            Self::OversizedRequest => "oversized_request",
            Self::OversizedResponse => "oversized_response",
            Self::UnusualHeaders => "unusual_headers",
            Self::Baseline => "baseline",
        }
    }

    pub fn parse(field: &str, raw: &str) -> Result<Self, FieldError> {
        Self::ALL
            .into_iter()
            .find(|trigger| trigger.as_str() == raw)
            .ok_or_else(|| FieldError::new(field, format!("unknown trigger {raw:?}")))
    }

    /// This trigger's bit in a [`TriggerSet`].
    pub fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// A set of triggers, stored as a bit mask so the database can filter on it
/// with a single `&`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TriggerSet(u32);

impl TriggerSet {
    pub const EMPTY: TriggerSet = TriggerSet(0);

    /// The triggers that are on by default: everything except `slow_p99`.
    pub fn defaults() -> Self {
        Trigger::ALL
            .into_iter()
            .filter(|trigger| *trigger != Trigger::SlowP99)
            .collect()
    }

    /// Rebuilds a set from stored bits; a bit that names no trigger is refused.
    pub fn from_bits(bits: u32) -> Option<Self> {
        let known = Trigger::ALL
            .iter()
            .fold(0, |all, trigger| all | trigger.bit());
        (bits & !known == 0).then_some(Self(bits))
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    pub fn contains(self, trigger: Trigger) -> bool {
        self.0 & trigger.bit() != 0
    }

    pub fn with(self, trigger: Trigger) -> Self {
        Self(self.0 | trigger.bit())
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The first trigger in priority order, if any.
    pub fn primary(self) -> Option<Trigger> {
        Trigger::ALL
            .into_iter()
            .find(|trigger| self.contains(*trigger))
    }

    /// The members, in priority order.
    pub fn iter(self) -> impl Iterator<Item = Trigger> {
        Trigger::ALL.into_iter().filter(move |t| self.contains(*t))
    }
}

impl FromIterator<Trigger> for TriggerSet {
    fn from_iter<I: IntoIterator<Item = Trigger>>(iter: I) -> Self {
        iter.into_iter().fold(Self::EMPTY, Self::with)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_names_are_refused() {
        for trigger in Trigger::ALL {
            assert_eq!(Trigger::parse("t", trigger.as_str()).unwrap(), trigger);
        }
        for bad in ["", "server_error", "Slow", "slow "] {
            assert_eq!(
                Trigger::parse("triggers", bad).unwrap_err().field,
                "triggers"
            );
        }
    }

    #[test]
    fn each_trigger_has_its_own_stable_bit() {
        let bits: Vec<u32> = Trigger::ALL.iter().map(|t| t.bit()).collect();
        assert_eq!(bits, [1, 2, 4, 8, 16, 32, 64, 128, 256]);
    }

    #[test]
    fn the_defaults_are_everything_but_the_relative_p99() {
        let defaults = TriggerSet::defaults();
        for trigger in Trigger::ALL {
            assert_eq!(
                defaults.contains(trigger),
                trigger != Trigger::SlowP99,
                "{trigger:?}"
            );
        }
        assert_eq!(defaults.bits(), 503);
    }

    #[test]
    fn stored_bits_with_an_unknown_trigger_are_refused() {
        assert!(TriggerSet::from_bits(511).is_some());
        assert!(TriggerSet::from_bits(512).is_none());
        assert!(TriggerSet::from_bits(1 << 31).is_none());
    }

    #[test]
    fn the_primary_trigger_is_the_first_in_priority_order() {
        let set: TriggerSet = [Trigger::Baseline, Trigger::Slow, Trigger::Retried]
            .into_iter()
            .collect();
        assert_eq!(set.primary(), Some(Trigger::Retried));
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Trigger::Retried, Trigger::Slow, Trigger::Baseline]
        );
        assert_eq!(TriggerSet::EMPTY.primary(), None);
        assert!(TriggerSet::EMPTY.is_empty());
    }
}
