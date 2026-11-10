//! The rustls server configuration shared by every HTTPS connection: the TLS
//! policy from the config file, ALPN, session tickets on and early data off
//! (ADR 0009). The certificate comes from a resolver the data plane supplies,
//! because choosing one needs the routing snapshot.

use std::sync::Arc;

use rustls::server::ResolvesServerCert;
use rustls::ServerConfig;
use thiserror::Error;

/// The lowest TLS version accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MinTlsVersion {
    #[default]
    V1_2,
    V1_3,
}

impl MinTlsVersion {
    /// Parses the config spelling `"1.2"` or `"1.3"`.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "1.2" => Some(Self::V1_2),
            "1.3" => Some(Self::V1_3),
            _ => None,
        }
    }

    fn versions(self) -> Vec<&'static rustls::SupportedProtocolVersion> {
        match self {
            Self::V1_2 => vec![&rustls::version::TLS12, &rustls::version::TLS13],
            Self::V1_3 => vec![&rustls::version::TLS13],
        }
    }
}

/// The process-wide TLS policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerTlsPolicy {
    pub min_version: MinTlsVersion,
    /// Offer `h2` through ALPN besides `http/1.1`.
    pub http2: bool,
}

impl Default for ServerTlsPolicy {
    fn default() -> Self {
        Self {
            min_version: MinTlsVersion::default(),
            http2: true,
        }
    }
}

/// Why the server configuration could not be built.
#[derive(Debug, Error)]
pub enum ServerConfigError {
    #[error("the TLS protocol versions are not supported by the crypto provider")]
    Versions,
    #[error("the session ticket key could not be generated")]
    Ticketer,
}

/// Builds the configuration: the policy's versions, ALPN from `http2`, session
/// tickets on, early data off, and `resolver` choosing the certificate.
pub fn build_server_config(
    policy: &ServerTlsPolicy,
    resolver: Arc<dyn ResolvesServerCert>,
) -> Result<ServerConfig, ServerConfigError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&policy.min_version.versions())
        .map_err(|_| ServerConfigError::Versions)?
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    config.alpn_protocols = if policy.http2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    config.ticketer =
        rustls::crypto::ring::Ticketer::new().map_err(|_| ServerConfigError::Ticketer)?;
    config.max_early_data_size = 0;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use fast_proxy_test_pki::{handshake, ClientHello, LeafSpec, TestCa};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::server::ClientHello as ServerHello;
    use rustls::sign::CertifiedKey;
    use rustls::ProtocolVersion;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;

    use super::*;

    #[derive(Debug)]
    struct One(Arc<CertifiedKey>);

    impl ResolvesServerCert for One {
        fn resolve(&self, _: ServerHello<'_>) -> Option<Arc<CertifiedKey>> {
            Some(Arc::clone(&self.0))
        }
    }

    async fn serve(policy: ServerTlsPolicy) -> (SocketAddr, TestCa) {
        let ca = TestCa::new("ca");
        let issued = ca.issue(&LeafSpec::new(&["a.example.com"]));
        let chain: Vec<CertificateDer<'static>> =
            CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap();
        let key = PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes()).unwrap();
        let signing = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
        let resolver = Arc::new(One(Arc::new(CertifiedKey::new(chain, signing))));
        let acceptor = TlsAcceptor::from(Arc::new(build_server_config(&policy, resolver).unwrap()));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let _ = acceptor.accept(stream).await;
                });
            }
        });
        (addr, ca)
    }

    fn hello(ca: &TestCa) -> ClientHello {
        ClientHello {
            sni: Some("a.example.com".into()),
            trust_pem: vec![ca.pem.clone()],
            alpn: vec!["h2".into(), "http/1.1".into()],
            ..ClientHello::default()
        }
    }

    #[tokio::test]
    async fn tls_12_and_13_are_accepted_by_default() {
        let (addr, ca) = serve(ServerTlsPolicy::default()).await;
        let modern = handshake(addr, &hello(&ca)).await.unwrap();
        assert_eq!(modern.version, Some(ProtocolVersion::TLSv1_3));
        let mut legacy = hello(&ca);
        legacy.only_version = Some(ProtocolVersion::TLSv1_2);
        let negotiated = handshake(addr, &legacy).await.unwrap();
        assert_eq!(negotiated.version, Some(ProtocolVersion::TLSv1_2));
    }

    #[tokio::test]
    async fn a_client_limited_to_12_is_refused_when_the_minimum_is_13() {
        let (addr, ca) = serve(ServerTlsPolicy {
            min_version: MinTlsVersion::V1_3,
            ..ServerTlsPolicy::default()
        })
        .await;
        let mut legacy = hello(&ca);
        legacy.only_version = Some(ProtocolVersion::TLSv1_2);
        assert!(handshake(addr, &legacy).await.is_err());
        let modern = handshake(addr, &hello(&ca)).await.unwrap();
        assert_eq!(modern.version, Some(ProtocolVersion::TLSv1_3));
    }

    #[tokio::test]
    async fn alpn_negotiates_h2_or_http11_and_h2_can_be_turned_off() {
        let (addr, ca) = serve(ServerTlsPolicy::default()).await;
        let negotiated = handshake(addr, &hello(&ca)).await.unwrap();
        assert_eq!(negotiated.alpn.as_deref(), Some(&b"h2"[..]));
        let mut http1_only = hello(&ca);
        http1_only.alpn = vec!["http/1.1".into()];
        let negotiated = handshake(addr, &http1_only).await.unwrap();
        assert_eq!(negotiated.alpn.as_deref(), Some(&b"http/1.1"[..]));

        let (addr, ca) = serve(ServerTlsPolicy {
            http2: false,
            ..ServerTlsPolicy::default()
        })
        .await;
        let negotiated = handshake(addr, &hello(&ca)).await.unwrap();
        assert_eq!(negotiated.alpn.as_deref(), Some(&b"http/1.1"[..]));
    }

    #[test]
    fn tickets_are_on_and_early_data_is_off() {
        let config = build_server_config(
            &ServerTlsPolicy::default(),
            Arc::new(One(
                crate::ephemeral_unknown_host(1_800_000_000_000).unwrap()
            )),
        )
        .unwrap();
        assert!(config.ticketer.enabled());
        assert_eq!(config.max_early_data_size, 0);
    }

    #[test]
    fn versions_parse_from_their_config_spelling() {
        assert_eq!(MinTlsVersion::parse("1.2"), Some(MinTlsVersion::V1_2));
        assert_eq!(MinTlsVersion::parse("1.3"), Some(MinTlsVersion::V1_3));
        for bad in ["1.1", "1", "TLS1.2", ""] {
            assert_eq!(MinTlsVersion::parse(bad), None, "{bad}");
        }
    }
}
