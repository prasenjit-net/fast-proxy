//! Choosing the certificate for a TLS handshake from the routing snapshot:
//! SNI to subsystem to its certificate (ADR 0010). The snapshot is loaded per
//! handshake, so a swap changes what the next handshake gets and never touches
//! one in flight.

use std::sync::Arc;

use fast_proxy_routing::{Snapshot, SnapshotHolder, SniLookup};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// What a handshake should present.
#[derive(Debug, Clone)]
pub(crate) enum Selection {
    /// The certificate of the subsystem the name belongs to.
    Subsystem(Arc<CertifiedKey>),
    /// The in-memory placeholder: the name is unknown, or its subsystem has no
    /// usable HTTPS certificate. The HTTP layer then answers 404 or 421.
    Placeholder,
    /// Fail the handshake (`reject_unknown_sni`).
    Reject,
}

/// Picks the certificate for `sni` (`None` when the client sent none).
pub(crate) fn select_certificate(
    snapshot: &Snapshot,
    sni: Option<&str>,
    reject_unknown_sni: bool,
) -> Selection {
    let unknown = || {
        if reject_unknown_sni {
            Selection::Reject
        } else {
            Selection::Placeholder
        }
    };
    let Some(name) = sni else {
        return unknown();
    };
    match snapshot.sni_lookup(name) {
        SniLookup::Unknown => unknown(),
        SniLookup::Subsystem { https, .. } => match &https.certificate {
            Some(handle) if snapshot.tls_enabled && https.enabled => {
                Selection::Subsystem(Arc::clone(&handle.key))
            }
            _ => Selection::Placeholder,
        },
    }
}

/// The rustls resolver over the live snapshot.
pub(crate) struct SnapshotCertResolver {
    snapshots: Arc<SnapshotHolder>,
    placeholder: Arc<CertifiedKey>,
    reject_unknown_sni: bool,
}

impl std::fmt::Debug for SnapshotCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotCertResolver")
            .field("reject_unknown_sni", &self.reject_unknown_sni)
            .finish_non_exhaustive()
    }
}

impl SnapshotCertResolver {
    pub(crate) fn new(
        snapshots: Arc<SnapshotHolder>,
        placeholder: Arc<CertifiedKey>,
        reject_unknown_sni: bool,
    ) -> Self {
        Self {
            snapshots,
            placeholder,
            reject_unknown_sni,
        }
    }
}

impl ResolvesServerCert for SnapshotCertResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let snapshot = self.snapshots.load();
        match select_certificate(
            &snapshot,
            client_hello.server_name(),
            self.reject_unknown_sni,
        ) {
            Selection::Subsystem(key) => Some(key),
            Selection::Placeholder => Some(Arc::clone(&self.placeholder)),
            Selection::Reject => None,
        }
    }
}
