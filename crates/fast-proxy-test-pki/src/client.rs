use std::net::SocketAddr;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ProtocolVersion, RootCertStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// How the test client presents itself.
#[derive(Debug, Clone, Default)]
pub struct ClientHello {
    /// The SNI to send; `None` connects without SNI (by IP).
    pub sni: Option<String>,
    /// CA certificates (PEM) the client trusts.
    pub trust_pem: Vec<String>,
    /// Offered ALPN protocols, for example `["h2", "http/1.1"]`.
    pub alpn: Vec<String>,
    /// Restricts the client to TLS 1.2 or 1.3 only when set.
    pub only_version: Option<ProtocolVersion>,
    /// Skip certificate verification (to inspect a placeholder certificate).
    pub insecure: bool,
}

/// What a successful handshake negotiated.
#[derive(Debug, Clone)]
pub struct Negotiated {
    pub version: Option<ProtocolVersion>,
    pub alpn: Option<Vec<u8>>,
    /// The DER of the certificate the server presented first.
    pub leaf_der: Vec<u8>,
}

/// A failed handshake, as text (tests assert only that it failed).
#[derive(Debug)]
pub struct HandshakeError(pub String);

/// Connects to `addr`, completes a TLS handshake as `hello` describes, and
/// reports what was negotiated.
pub async fn handshake(
    addr: SocketAddr,
    hello: &ClientHello,
) -> Result<Negotiated, HandshakeError> {
    let (mut tls, negotiated) = connect(addr, hello).await?;
    let _ = tls.shutdown().await;
    Ok(negotiated)
}

/// Completes a handshake, then sends `raw` (one HTTP/1.1 request that asks for
/// `Connection: close`) and returns everything the server answers.
pub async fn request(
    addr: SocketAddr,
    hello: &ClientHello,
    raw: &[u8],
) -> Result<String, HandshakeError> {
    let (mut tls, _) = connect(addr, hello).await?;
    let fail = |err: std::io::Error| HandshakeError(err.to_string());
    tls.write_all(raw).await.map_err(fail)?;
    let mut answer = Vec::new();
    // A server closing without a TLS close_notify is an error here but not for the answer.
    let _ = tls.read_to_end(&mut answer).await;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

async fn connect(
    addr: SocketAddr,
    hello: &ClientHello,
) -> Result<(tokio_rustls::client::TlsStream<TcpStream>, Negotiated), HandshakeError> {
    let fail = |err: &dyn std::fmt::Display| HandshakeError(err.to_string());
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let versions: Vec<&'static rustls::SupportedProtocolVersion> = match hello.only_version {
        Some(ProtocolVersion::TLSv1_2) => vec![&rustls::version::TLS12],
        Some(_) => vec![&rustls::version::TLS13],
        None => vec![&rustls::version::TLS12, &rustls::version::TLS13],
    };
    let builder = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&versions)
        .map_err(|err| fail(&err))?;
    let mut config = if hello.insecure {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnything))
            .with_no_client_auth()
    } else {
        let mut roots = RootCertStore::empty();
        for pem in &hello.trust_pem {
            for der in certificates_in(pem) {
                roots.add(der).map_err(|err| fail(&err))?;
            }
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    config.alpn_protocols = hello.alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    config.enable_sni = hello.sni.is_some();
    let name = hello.sni.clone().unwrap_or_else(|| addr.ip().to_string());
    let server_name = ServerName::try_from(name).map_err(|err| fail(&err))?;
    let stream = TcpStream::connect(addr).await.map_err(|err| fail(&err))?;
    stream.set_nodelay(true).map_err(|err| fail(&err))?;
    let tls = TlsConnector::from(Arc::new(config))
        .connect(server_name, stream)
        .await
        .map_err(|err| fail(&err))?;
    let (_, connection) = tls.get_ref();
    let negotiated = Negotiated {
        version: connection.protocol_version(),
        alpn: connection.alpn_protocol().map(<[u8]>::to_vec),
        leaf_der: connection
            .peer_certificates()
            .and_then(|certs| certs.first())
            .map(|der| der.as_ref().to_vec())
            .unwrap_or_default(),
    };
    Ok((tls, negotiated))
}

/// Decodes every certificate in a PEM string (the helpers in this crate only
/// ever produce well-formed PEM).
fn certificates_in(pem: &str) -> Vec<CertificateDer<'static>> {
    use rustls::pki_types::pem::PemObject;
    CertificateDer::pem_slice_iter(pem.as_bytes())
        .filter_map(Result::ok)
        .collect()
}

#[derive(Debug)]
struct AcceptAnything;

impl rustls::client::danger::ServerCertVerifier for AcceptAnything {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
