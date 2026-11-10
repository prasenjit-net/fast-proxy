use std::sync::Arc;

use fast_proxy_model::{CertificateId, Hsts, HttpsPolicy, RedirectMode};
use fast_proxy_tls::CertHandle;

/// A subsystem's HTTPS settings and its certificate, resolved at compile time.
#[derive(Debug, Clone)]
pub struct CompiledHttps {
    /// The subsystem opted into HTTPS (stored setting; effective only while TLS
    /// is on for the process, see [`Snapshot::tls_enabled`](crate::Snapshot)).
    pub enabled: bool,
    pub redirect: RedirectMode,
    pub hsts: Hsts,
    /// The attached certificate's id, even when it is unreadable.
    pub certificate_id: Option<CertificateId>,
    /// The parsed certificate; `None` when none is attached, or the attached one
    /// is unreadable (the handshake then falls back to the placeholder).
    pub certificate: Option<Arc<CertHandle>>,
}

impl CompiledHttps {
    pub(crate) fn new(policy: &HttpsPolicy, certificate: Option<Arc<CertHandle>>) -> Self {
        Self {
            enabled: policy.enabled,
            redirect: policy.redirect,
            hsts: policy.hsts,
            certificate_id: policy.certificate_id,
            certificate,
        }
    }
}
