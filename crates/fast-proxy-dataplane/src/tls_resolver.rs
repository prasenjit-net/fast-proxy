//! Choosing the certificate for a TLS handshake from the routing snapshot:
//! SNI to subsystem to its certificate (ADR 0010). The snapshot is loaded once
//! per handshake, so a swap changes what the next handshake gets and never
//! touches one in flight. The choice is made once and fixed for the connection,
//! so the 421 rule compares the certificate that was actually presented.

use std::sync::Arc;

use fast_proxy_model::CertificateId;
use fast_proxy_routing::{Snapshot, SniLookup};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// What a handshake should present.
#[derive(Debug, Clone)]
pub(crate) enum Selection {
    /// The certificate of the subsystem the name belongs to.
    Subsystem {
        id: CertificateId,
        key: Arc<CertifiedKey>,
    },
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
            Some(handle) if snapshot.tls_enabled && https.enabled => Selection::Subsystem {
                id: handle.id,
                key: Arc::clone(&handle.key),
            },
            _ => Selection::Placeholder,
        },
    }
}

/// Presents one fixed certificate: the one chosen for this connection.
#[derive(Debug)]
pub(crate) struct FixedCertificate(pub Arc<CertifiedKey>);

impl ResolvesServerCert for FixedCertificate {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// The resolver of the shared base configuration: it never answers, because
/// every connection replaces it with [`FixedCertificate`] before handshaking.
#[derive(Debug)]
pub(crate) struct NoCertificate;

impl ResolvesServerCert for NoCertificate {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        None
    }
}
