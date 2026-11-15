//! When to order a certificate, and what a failure or success does to the
//! status (K6, K7). Everything is computed from stored facts and `now_ms`, so a
//! restart never re-orders a valid certificate and never resets a backoff.

use fast_proxy_model::{sanitize_error_detail, AcmeState, AcmeStatus, CertificateId, SubsystemId};

use crate::FailureKind;

const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;

/// The renewal and backoff rules. [`Default`] holds the documented values;
/// tests may shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenewalPolicy {
    /// A certificate is due after this share of its lifetime, in thousandths.
    pub renew_after_permille: i64,
    /// Up to this share of the lifetime (thousandths) is taken off the due
    /// time, stable per subsystem, so a fleet does not renew in one minute.
    pub jitter_permille: i64,
    /// The wait after the 1st, 2nd, ... consecutive transient failure; the
    /// last entry repeats.
    pub backoff_ms: [i64; 5],
    /// Each backoff wait varies by up to this share (thousandths) either way.
    pub backoff_jitter_permille: i64,
    /// The window of the failure cap.
    pub cap_window_ms: i64,
    /// Most failed orders in one window, forced ones included.
    pub cap_failures: u32,
    /// The wait after a failure that cannot succeed without a change.
    pub permanent_wait_ms: i64,
    /// The wait after a rate-limit answer that named no time.
    pub rate_limited_wait_ms: i64,
    /// How long after the hosts changed to wait before ordering, so a bulk
    /// edit makes one order.
    pub names_changed_debounce_ms: i64,
}

impl Default for RenewalPolicy {
    fn default() -> Self {
        Self {
            renew_after_permille: 667,
            jitter_permille: 50,
            backoff_ms: [
                MINUTE_MS,
                5 * MINUTE_MS,
                30 * MINUTE_MS,
                2 * HOUR_MS,
                6 * HOUR_MS,
            ],
            backoff_jitter_permille: 100,
            cap_window_ms: HOUR_MS,
            cap_failures: 5,
            permanent_wait_ms: 6 * HOUR_MS,
            rate_limited_wait_ms: HOUR_MS,
            names_changed_debounce_ms: 30_000,
        }
    }
}

/// The attached ACME certificate, as far as renewal cares.
#[derive(Debug, Clone, Copy)]
pub struct CertificateFacts<'a> {
    pub names: &'a [String],
    pub not_before_ms: i64,
    pub not_after_ms: i64,
}

/// Everything [`RenewalPolicy::decide`] looks at for one subsystem.
#[derive(Debug, Clone, Copy)]
pub struct RenewalFacts<'a> {
    pub now_ms: i64,
    pub subsystem: SubsystemId,
    /// The names an order would cover now ([`crate::eligible_names`]).
    pub eligible: &'a [String],
    pub certificate: Option<CertificateFacts<'a>>,
    pub status: &'a AcmeStatus,
    /// An admin asked for a renewal now.
    pub forced: bool,
    /// When the subsystem's hosts last changed, if the runner saw it.
    pub names_changed_at_ms: Option<i64>,
}

/// Why an order is placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderReason {
    /// No ACME certificate is attached yet.
    Missing,
    /// A host was added that the certificate does not cover.
    NamesChanged,
    /// The certificate passed its renewal point.
    Due,
    /// An admin asked.
    Forced,
}

/// What to do for one subsystem now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// No eligible name: nothing to order.
    Idle,
    /// Place an order now.
    Order(OrderReason),
    /// Backing off or debouncing: look again at this time.
    WaitUntil(i64),
    /// The certificate is fine until it becomes due at this time.
    NotDueUntil(i64),
    /// A forced renewal was refused: the hourly cap is spent until this time.
    CapReached { until_ms: i64 },
}

impl RenewalPolicy {
    /// Decides for one subsystem. A forced renewal skips the backoff wait but
    /// never the hourly cap.
    pub fn decide(&self, facts: &RenewalFacts<'_>) -> Decision {
        let now = facts.now_ms;
        if facts.eligible.is_empty() {
            return Decision::Idle;
        }
        if facts.forced {
            return match self.cap_ends_at(facts.status, now) {
                Some(until_ms) => Decision::CapReached { until_ms },
                None => Decision::Order(OrderReason::Forced),
            };
        }
        if let Some(next) = facts.status.next_attempt_at_ms.filter(|next| *next > now) {
            return Decision::WaitUntil(next);
        }
        let Some(certificate) = facts.certificate else {
            return Decision::Order(OrderReason::Missing);
        };
        if !covers_all(certificate.names, facts.eligible) {
            let ready_at = facts
                .names_changed_at_ms
                .map_or(now, |changed| changed + self.names_changed_debounce_ms);
            if ready_at > now {
                return Decision::WaitUntil(ready_at);
            }
            return Decision::Order(OrderReason::NamesChanged);
        }
        let due = self.due_at(
            certificate.not_before_ms,
            certificate.not_after_ms,
            facts.subsystem,
        );
        if now >= due {
            return Decision::Order(OrderReason::Due);
        }
        Decision::NotDueUntil(due)
    }

    /// When a certificate valid from `not_before_ms` to `not_after_ms` is due:
    /// at [`renew_after_permille`](Self::renew_after_permille) of its lifetime,
    /// less a jitter that is stable for the subsystem.
    pub fn due_at(&self, not_before_ms: i64, not_after_ms: i64, subsystem: SubsystemId) -> i64 {
        let lifetime = not_after_ms.saturating_sub(not_before_ms).max(0);
        let renew_point = lifetime.saturating_mul(self.renew_after_permille) / 1000;
        let max_jitter = lifetime.saturating_mul(self.jitter_permille) / 1000;
        let jitter = max_jitter.saturating_mul(unit_permille(subsystem.0 as u64)) / 1000;
        not_before_ms.saturating_add(renew_point - jitter)
    }

    /// The end of the current cap window, when the cap is spent at `now_ms`.
    pub fn cap_ends_at(&self, status: &AcmeStatus, now_ms: i64) -> Option<i64> {
        let start = status.failure_window_start_ms?;
        let ends = start.saturating_add(self.cap_window_ms);
        (ends > now_ms && status.window_failures >= self.cap_failures).then_some(ends)
    }

    /// Marks an order as started: `pending` for a first certificate,
    /// `renewing` while a current one keeps serving.
    pub fn record_started(&self, status: &mut AcmeStatus, has_certificate: bool, now_ms: i64) {
        status.state = if has_certificate {
            AcmeState::Renewing
        } else {
            AcmeState::Pending
        };
        status.last_attempt_at_ms = Some(now_ms);
    }

    /// Records a failed order: counts it, schedules the next attempt by its
    /// kind and the cap, and keeps a sanitized `detail`.
    pub fn record_failure(
        &self,
        status: &mut AcmeStatus,
        kind: FailureKind,
        detail: &str,
        now_ms: i64,
    ) {
        status.failures = status.failures.saturating_add(1);
        let window_open = status
            .failure_window_start_ms
            .is_some_and(|start| now_ms - start < self.cap_window_ms);
        if window_open {
            status.window_failures = status.window_failures.saturating_add(1);
        } else {
            status.failure_window_start_ms = Some(now_ms);
            status.window_failures = 1;
        }
        let wait = match kind {
            FailureKind::Transient => self.backoff(status.subsystem_id, status.failures),
            FailureKind::Permanent => self.permanent_wait_ms,
            FailureKind::RateLimited { retry_at_ms } => retry_at_ms
                .map_or(self.rate_limited_wait_ms, |at| at - now_ms)
                .max(self.backoff_ms[0]),
        };
        let mut next = now_ms.saturating_add(wait);
        if let Some(cap_end) = self.cap_ends_at(status, now_ms) {
            next = next.max(cap_end);
        }
        status.next_attempt_at_ms = Some(next);
        status.state = match kind {
            FailureKind::RateLimited { .. } => AcmeState::RateLimited,
            FailureKind::Transient | FailureKind::Permanent => AcmeState::Failed,
        };
        status.last_error = sanitize_error_detail(detail);
        status.last_attempt_at_ms = Some(now_ms);
    }

    /// Records an issued certificate: the backoff resets (the cap window does
    /// not: it counts failures, and a success is not one).
    pub fn record_success(&self, status: &mut AcmeStatus, certificate: CertificateId, now_ms: i64) {
        status.state = AcmeState::Valid;
        status.certificate_id = Some(certificate);
        status.failures = 0;
        status.next_attempt_at_ms = None;
        status.last_error.clear();
        status.last_attempt_at_ms = Some(now_ms);
        status.last_success_at_ms = Some(now_ms);
    }

    /// The jittered wait after the `failures`-th consecutive transient failure.
    fn backoff(&self, subsystem: SubsystemId, failures: u32) -> i64 {
        let step = (failures.max(1) as usize - 1).min(self.backoff_ms.len() - 1);
        let base = self.backoff_ms[step];
        let spread = base.saturating_mul(self.backoff_jitter_permille) / 1000;
        let seed = (subsystem.0 as u64)
            .wrapping_mul(31)
            .wrapping_add(u64::from(failures));
        // Maps [0, 1000) onto [-spread, +spread).
        let offset = spread.saturating_mul(2 * unit_permille(seed) - 1000) / 1000;
        base + offset
    }
}

/// Whether every eligible name is in the certificate's names.
fn covers_all(certificate_names: &[String], eligible: &[String]) -> bool {
    eligible.iter().all(|name| certificate_names.contains(name))
}

/// A well-mixed value in `[0, 1000)` from `seed` (SplitMix64), so jitter is
/// spread out yet the same on every instance and after every restart.
fn unit_permille(seed: u64) -> i64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z % 1000) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    struct Case {
        status: AcmeStatus,
        eligible: Vec<String>,
        cert_names: Vec<String>,
        has_certificate: bool,
    }

    impl Case {
        fn new() -> Self {
            Self {
                status: AcmeStatus::new(SubsystemId(1)),
                eligible: names(&["shop.example.com"]),
                cert_names: names(&["shop.example.com"]),
                has_certificate: true,
            }
        }

        fn decide(&self, now_ms: i64, forced: bool, changed: Option<i64>) -> Decision {
            let certificate = self.has_certificate.then_some(CertificateFacts {
                names: &self.cert_names,
                not_before_ms: 0,
                not_after_ms: 90 * DAY,
            });
            RenewalPolicy::default().decide(&RenewalFacts {
                now_ms,
                subsystem: SubsystemId(1),
                eligible: &self.eligible,
                certificate,
                status: &self.status,
                forced,
                names_changed_at_ms: changed,
            })
        }
    }

    #[test]
    fn nothing_eligible_is_idle_even_when_forced() {
        let mut case = Case::new();
        case.eligible.clear();
        assert_eq!(case.decide(0, true, None), Decision::Idle);
    }

    #[test]
    fn a_missing_certificate_is_ordered_at_once() {
        let mut case = Case::new();
        case.has_certificate = false;
        assert_eq!(
            case.decide(5, false, None),
            Decision::Order(OrderReason::Missing)
        );
    }

    #[test]
    fn a_ninety_day_certificate_is_due_around_day_sixty() {
        let case = Case::new();
        let due = RenewalPolicy::default().due_at(0, 90 * DAY, SubsystemId(1));
        assert!((55 * DAY..=61 * DAY).contains(&due), "{due}");
        assert_eq!(
            case.decide(due - 1, false, None),
            Decision::NotDueUntil(due)
        );
        assert_eq!(
            case.decide(due, false, None),
            Decision::Order(OrderReason::Due)
        );
    }

    #[test]
    fn the_jitter_differs_between_subsystems_but_not_between_calls() {
        let policy = RenewalPolicy::default();
        let dues: Vec<i64> = (1..=20)
            .map(|id| policy.due_at(0, 90 * DAY, SubsystemId(id)))
            .collect();
        let distinct: std::collections::HashSet<_> = dues.iter().collect();
        assert!(distinct.len() > 10, "{dues:?}");
        assert_eq!(dues[3], policy.due_at(0, 90 * DAY, SubsystemId(4)));
    }

    #[test]
    fn an_added_name_is_ordered_after_the_debounce() {
        let mut case = Case::new();
        case.eligible.push("www.shop.example.com".into());
        assert_eq!(
            case.decide(1_000, false, Some(1_000)),
            Decision::WaitUntil(31_000)
        );
        assert_eq!(
            case.decide(31_000, false, Some(1_000)),
            Decision::Order(OrderReason::NamesChanged)
        );
        assert_eq!(
            case.decide(1_000, false, None),
            Decision::Order(OrderReason::NamesChanged),
            "a change the runner did not see is not debounced"
        );
    }

    #[test]
    fn a_removed_name_does_not_force_an_order() {
        let mut case = Case::new();
        case.cert_names.push("old.example.com".into());
        assert!(matches!(
            case.decide(DAY, false, Some(0)),
            Decision::NotDueUntil(_)
        ));
    }

    #[test]
    fn a_scheduled_attempt_waits_unless_forced() {
        let mut case = Case::new();
        case.status.next_attempt_at_ms = Some(500);
        assert_eq!(case.decide(100, false, None), Decision::WaitUntil(500));
        assert_eq!(
            case.decide(100, true, None),
            Decision::Order(OrderReason::Forced)
        );
    }

    #[test]
    fn transient_failures_climb_the_ladder_within_ten_percent() {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(7));
        let mut now = 0;
        for (index, base) in policy
            .backoff_ms
            .iter()
            .chain([&policy.backoff_ms[4]])
            .enumerate()
        {
            // Far apart, so the hourly cap never applies.
            now += 10 * HOUR_MS;
            policy.record_failure(&mut status, FailureKind::Transient, "boom", now);
            let wait = status.next_attempt_at_ms.unwrap() - now;
            assert!(
                (wait - base).abs() <= base / 10,
                "failure {}: {wait} vs {base}",
                index + 1
            );
            assert_eq!(status.state, AcmeState::Failed);
            assert_eq!(status.failures as usize, index + 1);
        }
    }

    #[test]
    fn permanent_and_rate_limited_failures_wait_long_or_as_told() {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(1));
        policy.record_failure(&mut status, FailureKind::Permanent, "caa", 0);
        assert_eq!(status.next_attempt_at_ms, Some(6 * HOUR_MS));

        let mut status = AcmeStatus::new(SubsystemId(1));
        let told = FailureKind::RateLimited {
            retry_at_ms: Some(3 * HOUR_MS),
        };
        policy.record_failure(&mut status, told, "slow down", 0);
        assert_eq!(status.next_attempt_at_ms, Some(3 * HOUR_MS));
        assert_eq!(status.state, AcmeState::RateLimited);

        let mut status = AcmeStatus::new(SubsystemId(1));
        let untold = FailureKind::RateLimited { retry_at_ms: None };
        policy.record_failure(&mut status, untold, "", 0);
        assert_eq!(status.next_attempt_at_ms, Some(HOUR_MS));

        let mut status = AcmeStatus::new(SubsystemId(1));
        let past = FailureKind::RateLimited {
            retry_at_ms: Some(-5),
        };
        policy.record_failure(&mut status, past, "", 0);
        assert_eq!(
            status.next_attempt_at_ms,
            Some(MINUTE_MS),
            "never sooner than a minute"
        );
    }

    #[test]
    fn the_fifth_failure_in_an_hour_waits_for_the_window_and_refuses_force() {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(1));
        for minute in 0..5 {
            policy.record_failure(&mut status, FailureKind::Transient, "x", minute * MINUTE_MS);
        }
        assert_eq!(status.window_failures, 5);
        assert!(status.next_attempt_at_ms.unwrap() >= HOUR_MS);
        assert_eq!(policy.cap_ends_at(&status, 10 * MINUTE_MS), Some(HOUR_MS));
        assert_eq!(policy.cap_ends_at(&status, HOUR_MS), None);

        let mut case = Case::new();
        case.status = status;
        assert_eq!(
            case.decide(10 * MINUTE_MS, true, None),
            Decision::CapReached { until_ms: HOUR_MS }
        );
    }

    #[test]
    fn a_failure_after_the_window_starts_a_new_one() {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(1));
        policy.record_failure(&mut status, FailureKind::Transient, "x", 0);
        policy.record_failure(&mut status, FailureKind::Transient, "x", HOUR_MS);
        assert_eq!(status.failure_window_start_ms, Some(HOUR_MS));
        assert_eq!(status.window_failures, 1);
        assert_eq!(status.failures, 2);
    }

    #[test]
    fn success_resets_the_backoff_and_error_and_start_marks_the_state() {
        let policy = RenewalPolicy::default();
        let mut status = AcmeStatus::new(SubsystemId(1));
        policy.record_started(&mut status, false, 1);
        assert_eq!(status.state, AcmeState::Pending);
        policy.record_failure(&mut status, FailureKind::Transient, "line one\nline two", 2);
        assert_eq!(status.last_error, "line one line two");
        policy.record_started(&mut status, true, 3);
        assert_eq!(status.state, AcmeState::Renewing);
        policy.record_success(&mut status, CertificateId(9), 4);
        assert_eq!(status.state, AcmeState::Valid);
        assert_eq!(status.certificate_id, Some(CertificateId(9)));
        assert_eq!((status.failures, status.next_attempt_at_ms), (0, None));
        assert!(status.last_error.is_empty());
        assert_eq!(status.last_success_at_ms, Some(4));
        assert_eq!(
            status.window_failures, 1,
            "the cap still counts the failure"
        );
    }

    #[test]
    fn a_certificate_with_no_lifetime_is_due_at_its_start() {
        let policy = RenewalPolicy::default();
        assert_eq!(policy.due_at(100, 100, SubsystemId(1)), 100);
        assert_eq!(policy.due_at(100, 50, SubsystemId(1)), 100);
    }
}
