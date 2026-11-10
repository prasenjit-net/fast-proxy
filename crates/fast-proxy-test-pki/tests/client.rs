use std::net::SocketAddr;
use std::sync::Arc;

use fast_proxy_test_pki::{handshake, ClientHello, LeafSpec, TestCa};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ProtocolVersion;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

async fn server(names: &[&str], ca: &TestCa, alpn: &[&str]) -> (SocketAddr, Vec<u8>) {
    let issued = ca.issue(&LeafSpec::new(names));
    let chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap();
    let leaf = chain[0].as_ref().to_vec();
    let key = PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes()).unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(chain, key)
    .unwrap();
    config.alpn_protocols = alpn.iter().map(|p| p.as_bytes().to_vec()).collect();
    let acceptor = TlsAcceptor::from(Arc::new(config));
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
    (addr, leaf)
}

fn hello(ca: &TestCa) -> ClientHello {
    ClientHello {
        sni: Some("a.example.com".into()),
        trust_pem: vec![ca.pem.clone()],
        ..ClientHello::default()
    }
}

#[tokio::test]
async fn a_trusted_server_reports_version_alpn_and_leaf() {
    let ca = TestCa::new("ca");
    let (addr, leaf) = server(&["a.example.com"], &ca, &["h2", "http/1.1"]).await;
    let mut client = hello(&ca);
    client.alpn = vec!["h2".into()];
    let negotiated = handshake(addr, &client).await.unwrap();
    assert_eq!(negotiated.alpn.as_deref(), Some(&b"h2"[..]));
    assert_eq!(negotiated.leaf_der, leaf);
    assert_eq!(negotiated.version, Some(ProtocolVersion::TLSv1_3));
}

#[tokio::test]
async fn the_client_can_be_limited_to_tls_12() {
    let ca = TestCa::new("ca");
    let (addr, _) = server(&["a.example.com"], &ca, &[]).await;
    let mut client = hello(&ca);
    client.only_version = Some(ProtocolVersion::TLSv1_2);
    let negotiated = handshake(addr, &client).await.unwrap();
    assert_eq!(negotiated.version, Some(ProtocolVersion::TLSv1_2));
    assert!(negotiated.alpn.is_none());
}

#[tokio::test]
async fn untrusted_or_wrong_name_fails_unless_insecure() {
    let ca = TestCa::new("ca");
    let (addr, _) = server(&["a.example.com"], &ca, &[]).await;
    let mut wrong_name = hello(&ca);
    wrong_name.sni = Some("b.example.com".into());
    assert!(handshake(addr, &wrong_name).await.is_err());
    let untrusted = ClientHello {
        sni: Some("a.example.com".into()),
        ..ClientHello::default()
    };
    assert!(handshake(addr, &untrusted).await.is_err());
    let insecure = ClientHello {
        insecure: true,
        ..untrusted
    };
    assert!(handshake(addr, &insecure).await.is_ok());
    let no_sni = ClientHello {
        sni: None,
        insecure: true,
        ..ClientHello::default()
    };
    assert!(handshake(addr, &no_sni).await.is_ok());
}
