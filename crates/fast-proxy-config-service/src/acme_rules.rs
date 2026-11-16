//! The rules for a subsystem's ACME switch (K4, ADR 0027). They are about
//! *changes*, not about a stored state, so they are checked where a change is
//! made rather than in compile: a subsystem whose stored switch is on must
//! keep compiling when ACME is later turned off for the process (it is then
//! inert), and the ACME runner itself must be able to attach what it issued.

use fast_proxy_model::{FieldError, HttpsPolicy};

use crate::ConfigService;

/// Checks an admin's change of HTTPS settings from `current` (`None` for a new
/// subsystem) to `next`, with ACME `available` for the process or not:
///
/// - turning the switch on needs ACME on for the process;
/// - while the switch stays on, the attached certificate is ACME's to manage,
///   so attaching or detaching another is refused.
pub(crate) fn check_acme_change(
    available: bool,
    current: Option<&HttpsPolicy>,
    next: &HttpsPolicy,
) -> Result<(), FieldError> {
    let was_on = current.is_some_and(|policy| policy.acme_enabled);
    if next.acme_enabled && !was_on && !available {
        return Err(FieldError::new(
            "https.acmeEnabled",
            "ACME is not enabled for this proxy ([data_plane.acme] enabled)",
        ));
    }
    let managed = current.filter(|policy| policy.acme_enabled && next.acme_enabled);
    if managed.is_some_and(|policy| policy.certificate_id != next.certificate_id) {
        return Err(FieldError::new(
            "https.certificateId",
            "is managed by ACME while https.acmeEnabled is on; turn ACME off to attach another certificate",
        ));
    }
    Ok(())
}

impl ConfigService {
    /// Whether ACME is on for this process (`[data_plane.acme] enabled`).
    pub fn acme_available(&self) -> bool {
        self.tls.settings.acme_enabled
    }

    /// The ACME switch rules for an admin's change of HTTPS settings (see
    /// [`check_acme_change`]); `current` is `None` for a new subsystem.
    pub fn check_https_change(
        &self,
        current: Option<&HttpsPolicy>,
        next: &HttpsPolicy,
    ) -> Result<(), FieldError> {
        check_acme_change(self.acme_available(), current, next)
    }
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::CertificateId;

    use super::*;

    fn policy(acme_enabled: bool, certificate: Option<i64>) -> HttpsPolicy {
        HttpsPolicy {
            enabled: true,
            certificate_id: certificate.map(CertificateId),
            acme_enabled,
            ..HttpsPolicy::default()
        }
    }

    #[test]
    fn turning_acme_on_needs_it_for_the_process() {
        let on = policy(true, Some(1));
        let off = policy(false, Some(1));
        let err = check_acme_change(false, Some(&off), &on).unwrap_err();
        assert_eq!(err.field, "https.acmeEnabled");
        assert_eq!(
            check_acme_change(false, None, &on).unwrap_err().field,
            "https.acmeEnabled"
        );
        assert!(check_acme_change(true, Some(&off), &on).is_ok());
        assert!(check_acme_change(true, None, &on).is_ok());
    }

    #[test]
    fn a_switch_already_on_stays_allowed_when_the_process_turned_acme_off() {
        let on = policy(true, Some(1));
        assert!(check_acme_change(false, Some(&on), &on).is_ok());
        assert!(check_acme_change(false, Some(&on), &policy(false, Some(1))).is_ok());
    }

    #[test]
    fn the_certificate_is_acme_managed_while_the_switch_stays_on() {
        let on = policy(true, Some(1));
        for other in [Some(2), None] {
            let err = check_acme_change(true, Some(&on), &policy(true, other)).unwrap_err();
            assert_eq!(err.field, "https.certificateId", "{other:?}");
        }
        // Turning ACME on with any certificate, or off while changing it, is fine.
        assert!(
            check_acme_change(true, Some(&policy(false, Some(1))), &policy(true, Some(2))).is_ok()
        );
        assert!(check_acme_change(true, Some(&on), &policy(false, Some(2))).is_ok());
    }
}
