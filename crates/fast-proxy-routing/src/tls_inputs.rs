use std::collections::HashSet;

use fast_proxy_tls::CertCache;

/// What the compile step needs besides the configuration to carry TLS into the
/// snapshot: the process switch, the parsed certificates, and the names of the
/// uploaded CA bundles. Taking them as inputs is what lets one atomic swap
/// change routing, HTTPS settings and certificates together (ADR 0011).
#[derive(Debug, Clone, Default)]
pub struct TlsInputs {
    /// Whether TLS is switched on for the process. With it off, subsystem HTTPS
    /// settings stay stored but inert and are not checked against certificates.
    pub enabled: bool,
    pub certificates: CertCache,
    /// Names of the stored CA bundles that upstream groups may reference.
    pub ca_bundles: HashSet<String>,
}

impl TlsInputs {
    /// TLS off, no certificates, no bundles.
    pub fn off() -> Self {
        Self::default()
    }
}
