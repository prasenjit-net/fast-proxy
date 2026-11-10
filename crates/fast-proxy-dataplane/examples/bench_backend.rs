//! A minimal, fast HTTP/1.1 backend for benchmarking the proxy: every request
//! gets the same fixed response. Not part of the product.
//!
//! ```text
//! cargo run --release -p fast-proxy-dataplane --example bench_backend -- 127.0.0.1:3001 [body_bytes] [--tls CA_PEM_OUT]
//! ```
//!
//! With `--tls` the backend speaks TLS with a certificate for `backend.internal`
//! issued by a CA it generates; the CA certificate is written to `CA_PEM_OUT` so it
//! can be uploaded as a CA bundle.

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http::Response;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// A TLS acceptor for `backend.internal`, writing the issuing CA to `ca_out`.
fn tls_acceptor(ca_out: &str) -> Result<tokio_rustls::TlsAcceptor, Box<dyn std::error::Error>> {
    use fast_proxy_test_pki::{LeafSpec, TestCa};
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let ca = TestCa::new("bench ca");
    let issued = ca.issue(&LeafSpec::new(&["backend.internal"]));
    std::fs::write(ca_out, &ca.pem)?;
    let chain = CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()?;
    let key = PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes())?;
    let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(chain, key)?;
    Ok(tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(config)))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let addr: SocketAddr = args
        .next()
        .ok_or("usage: bench_backend ADDR [BODY_BYTES] [--tls CA_PEM_OUT]")?
        .parse()?;
    let mut body_bytes = 2usize;
    let mut acceptor = None;
    while let Some(arg) = args.next() {
        if arg == "--tls" {
            let ca_out = args
                .next()
                .ok_or("--tls needs the path to write the CA to")?;
            acceptor = Some(tls_acceptor(&ca_out)?);
        } else {
            body_bytes = arg.parse()?;
        }
    }
    let body = Bytes::from(vec![b'x'; body_bytes]);

    let listener = TcpListener::bind(addr).await?;
    eprintln!("bench backend on http://{addr}, {body_bytes}-byte responses");
    loop {
        let (stream, _) = listener.accept().await?;
        let body = body.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let service = service_fn(move |_request| {
                let body = body.clone();
                async move { Ok::<_, Infallible>(Response::new(Full::new(body))) }
            });
            let http = hyper::server::conn::http1::Builder::new();
            match acceptor {
                Some(acceptor) => {
                    if let Ok(tls) = acceptor.accept(stream).await {
                        let _ = http.serve_connection(TokioIo::new(tls), service).await;
                    }
                }
                None => {
                    let _ = http.serve_connection(TokioIo::new(stream), service).await;
                }
            }
        });
    }
}
