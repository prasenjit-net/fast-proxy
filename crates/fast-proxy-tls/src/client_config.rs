//! rustls client configurations for TLS to upstreams: the OS trust store, an
//! uploaded CA bundle, or no verification at all (ADR 0013). Built once per
//! `(verify, bundle version)` and cached; replacing a bundle makes the next
//! lookup build a new one, which new connections pick up.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, OnceLock};

use fast_proxy_model::FieldError;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use thiserror::Error;

/// An uploaded CA bundle as the client configuration needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaBundle {
    pub name: String,
    pub pem: String,
    /// When the bundle last changed; a new value means a new configuration.
    pub updated_at_ms: i64,
}

/// How to verify an upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamTrust {
    /// Verify the certificate and name; off accepts anything (with a warning).
    pub verify: bool,
    /// An uploaded bundle; `None` uses the OS trust store.
    pub ca: Option<CaBundle>,
}

/// Why a client configuration could not be built.
#[derive(Debug, Error)]
pub enum ClientConfigError {
    #[error("the CA bundle {0} holds no usable certificate")]
    EmptyBundle(String),
    #[error("the TLS client configuration is invalid")]
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientKey {
    verify: bool,
    bundle: Option<(String, i64)>,
}

/// A cache of client configurations.
#[derive(Default)]
pub struct ClientConfigs {
    configs: Mutex<HashMap<ClientKey, Arc<ClientConfig>>>,
    native_roots: OnceLock<Arc<RootCertStore>>,
}

impl std::fmt::Debug for ClientConfigs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientConfigs").finish_non_exhaustive()
    }
}

impl ClientConfigs {
    pub fn new() -> Self {
        Self::default()
    }

    /// The configuration for `trust`, built on first use. A changed bundle
    /// replaces the earlier configuration for the same bundle name.
    pub fn get(&self, trust: &UpstreamTrust) -> Result<Arc<ClientConfig>, ClientConfigError> {
        let key = ClientKey {
            verify: trust.verify,
            bundle: trust
                .ca
                .as_ref()
                .map(|ca| (ca.name.clone(), ca.updated_at_ms)),
        };
        let mut configs = self
            .configs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(config) = configs.get(&key) {
            return Ok(Arc::clone(config));
        }
        let config = Arc::new(self.build(trust)?);
        if let Some((name, _)) = &key.bundle {
            configs.retain(|other, _| other.bundle.as_ref().is_none_or(|(n, _)| n != name));
        }
        configs.insert(key, Arc::clone(&config));
        Ok(config)
    }

    fn build(&self, trust: &UpstreamTrust) -> Result<ClientConfig, ClientConfigError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ClientConfig::builder_with_provider(Arc::clone(&provider))
            .with_safe_default_protocol_versions()
            .map_err(|_| ClientConfigError::Invalid)?;
        let config = if trust.verify {
            builder
                .with_root_certificates(self.roots(trust.ca.as_ref())?)
                .with_no_client_auth()
        } else {
            tracing::warn!("building an upstream TLS client that does not verify certificates");
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerification { provider }))
                .with_no_client_auth()
        };
        // No ALPN here: the connector that uses this configuration sets
        // `http/1.1` itself (upstream connections are HTTP/1.1, ADR 0009).
        Ok(config)
    }

    fn roots(&self, ca: Option<&CaBundle>) -> Result<RootCertStore, ClientConfigError> {
        match ca {
            Some(bundle) => roots_from_pem(bundle),
            None => Ok((**self.native_roots.get_or_init(load_native_roots)).clone()),
        }
    }
}

fn roots_from_pem(bundle: &CaBundle) -> Result<RootCertStore, ClientConfigError> {
    use rustls::pki_types::pem::PemObject;
    let mut roots = RootCertStore::empty();
    let certificates = CertificateDer::pem_slice_iter(bundle.pem.as_bytes()).filter_map(Result::ok);
    let (added, _) = roots.add_parsable_certificates(certificates);
    if added == 0 {
        return Err(ClientConfigError::EmptyBundle(bundle.name.clone()));
    }
    Ok(roots)
}

fn load_native_roots() -> Arc<RootCertStore> {
    let mut roots = RootCertStore::empty();
    let loaded = rustls_native_certs::load_native_certs();
    for error in &loaded.errors {
        tracing::warn!(%error, "some OS trust store certificates could not be loaded");
    }
    let (added, ignored) = roots.add_parsable_certificates(loaded.certs);
    tracing::debug!(added, ignored, "loaded the OS trust store");
    Arc::new(roots)
}

/// The name to use for SNI and the certificate check: the group's override
/// when set, else the endpoint's host (a DNS name or an IP address).
pub fn server_name_for(
    override_name: &str,
    endpoint_host: &str,
) -> Result<ServerName<'static>, FieldError> {
    let chosen = if override_name.is_empty() {
        endpoint_host
    } else {
        override_name
    };
    let host = chosen.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(ServerName::IpAddress(ip.into()));
    }
    ServerName::try_from(host.to_string())
        .map_err(|_| FieldError::new("tls.serverName", "is not a valid server name"))
}

#[derive(Debug)]
struct NoVerification {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use fast_proxy_test_pki::{Issued, LeafSpec, TestCa};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::PrivateKeyDer;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    use super::*;

    async fn serve(issued: &Issued) -> SocketAddr {
        let chain = CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let mut byte = [0u8; 1];
                        let _ = tls.read(&mut byte).await;
                        let _ = tls.write_all(b"ok").await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        addr
    }

    async fn connect(
        configs: &ClientConfigs,
        trust: &UpstreamTrust,
        addr: SocketAddr,
        name: ServerName<'static>,
    ) -> bool {
        let config = configs.get(trust).unwrap();
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let Ok(mut tls) = TlsConnector::from(config).connect(name, stream).await else {
            return false;
        };
        tls.write_all(b"x").await.unwrap();
        let mut reply = [0u8; 2];
        tls.read_exact(&mut reply).await.is_ok() && &reply == b"ok"
    }

    fn bundle(name: &str, pem: &str, updated: i64) -> Option<CaBundle> {
        Some(CaBundle {
            name: name.into(),
            pem: pem.into(),
            updated_at_ms: updated,
        })
    }

    fn trust(ca: Option<CaBundle>) -> UpstreamTrust {
        UpstreamTrust { verify: true, ca }
    }

    fn name(text: &str) -> ServerName<'static> {
        server_name_for("", text).unwrap()
    }

    #[tokio::test]
    async fn a_certificate_from_the_named_ca_is_trusted() {
        let ca = TestCa::new("internal");
        let issued = ca.issue(&LeafSpec::new(&["backend.internal"]));
        let addr = serve(&issued).await;
        let configs = ClientConfigs::new();
        let trusted = trust(bundle("internal", &ca.pem, 1));
        assert!(connect(&configs, &trusted, addr, name("backend.internal")).await);
    }

    #[tokio::test]
    async fn an_untrusted_ca_and_a_wrong_name_are_refused() {
        let ca = TestCa::new("internal");
        let issued = ca.issue(&LeafSpec::new(&["backend.internal"]));
        let addr = serve(&issued).await;
        let configs = ClientConfigs::new();
        let other = TestCa::new("someone else");
        let untrusted = trust(bundle("other", &other.pem, 1));
        assert!(!connect(&configs, &untrusted, addr, name("backend.internal")).await);
        let trusted = trust(bundle("internal", &ca.pem, 1));
        assert!(!connect(&configs, &trusted, addr, name("wrong.internal")).await);
    }

    #[tokio::test]
    async fn verification_off_accepts_an_untrusted_certificate() {
        let issued = TestCa::new("anyone").issue(&LeafSpec::new(&["backend.internal"]));
        let addr = serve(&issued).await;
        let configs = ClientConfigs::new();
        let unverified = UpstreamTrust {
            verify: false,
            ca: None,
        };
        assert!(connect(&configs, &unverified, addr, name("wrong.name")).await);
    }

    #[tokio::test]
    async fn an_ip_endpoint_is_checked_against_its_ip_or_an_override_name() {
        let ca = TestCa::new("internal");
        let by_ip = serve(&ca.issue(&LeafSpec::new(&["127.0.0.1"]))).await;
        let by_name = serve(&ca.issue(&LeafSpec::new(&["backend.internal"]))).await;
        let configs = ClientConfigs::new();
        let trusted = trust(bundle("internal", &ca.pem, 1));
        assert!(connect(&configs, &trusted, by_ip, name("127.0.0.1")).await);
        assert!(!connect(&configs, &trusted, by_name, name("127.0.0.1")).await);
        let overridden = server_name_for("backend.internal", "127.0.0.1").unwrap();
        assert!(connect(&configs, &trusted, by_name, overridden).await);
    }

    #[tokio::test]
    async fn a_replaced_bundle_takes_effect_for_new_connections() {
        let old = TestCa::new("old");
        let new = TestCa::new("new");
        let addr = serve(&new.issue(&LeafSpec::new(&["backend.internal"]))).await;
        let configs = ClientConfigs::new();
        let before = trust(bundle("internal", &old.pem, 1));
        assert!(!connect(&configs, &before, addr, name("backend.internal")).await);
        let after = trust(bundle("internal", &new.pem, 2));
        assert!(connect(&configs, &after, addr, name("backend.internal")).await);
        assert_eq!(
            configs.configs.lock().unwrap().len(),
            1,
            "the stale entry was dropped"
        );
    }

    #[test]
    fn configurations_are_cached_per_trust() {
        let ca = TestCa::new("internal");
        let configs = ClientConfigs::new();
        let trusted = trust(bundle("internal", &ca.pem, 1));
        let first = configs.get(&trusted).unwrap();
        assert!(Arc::ptr_eq(&first, &configs.get(&trusted).unwrap()));
        let unverified = UpstreamTrust {
            verify: false,
            ca: None,
        };
        assert!(!Arc::ptr_eq(&first, &configs.get(&unverified).unwrap()));
        assert!(first.alpn_protocols.is_empty(), "the connector sets ALPN");
    }

    #[test]
    fn an_empty_bundle_is_an_error_and_the_os_store_loads() {
        let configs = ClientConfigs::new();
        let empty = trust(bundle("empty", "no certificates here", 1));
        assert!(
            matches!(configs.get(&empty), Err(ClientConfigError::EmptyBundle(n)) if n == "empty")
        );
        assert!(configs.get(&trust(None)).is_ok());
    }

    #[test]
    fn server_names_come_from_the_override_or_the_host() {
        assert!(matches!(
            server_name_for("", "backend.internal"),
            Ok(ServerName::DnsName(n)) if n.as_ref() == "backend.internal"
        ));
        assert!(matches!(
            server_name_for("other.internal", "10.0.0.1"),
            Ok(ServerName::DnsName(n)) if n.as_ref() == "other.internal"
        ));
        assert!(matches!(
            server_name_for("", "10.0.0.1"),
            Ok(ServerName::IpAddress(_))
        ));
        assert!(matches!(
            server_name_for("", "[::1]"),
            Ok(ServerName::IpAddress(_))
        ));
        assert_eq!(
            server_name_for("bad name!", "x").unwrap_err().field,
            "tls.serverName"
        );
    }
}
