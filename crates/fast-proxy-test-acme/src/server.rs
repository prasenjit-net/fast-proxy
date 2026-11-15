//! The HTTPS listener: a loopback port, a certificate from a test CA, and
//! every request handed to [`crate::protocol::handle`].

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State as Extract;
use axum::http::{Method, Uri};
use axum::response::Response;
use axum::Router;
use fast_proxy_test_pki::{LeafSpec, TestCa};
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

use crate::state::State;

pub(crate) type Shared = Arc<Mutex<State>>;

pub(crate) async fn start(
    tls_ca: &TestCa,
    issuer: TestCa,
) -> (SocketAddr, tokio::task::JoinHandle<()>, Shared) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("binding the test ACME server");
    let addr = listener.local_addr().expect("a bound address");
    let state: Shared = Arc::new(Mutex::new(State::new(format!("https://{addr}"), issuer)));
    let router = Router::new().fallback(dispatch).with_state(state.clone());
    let acceptor = TlsAcceptor::from(Arc::new(server_config(tls_ca)));
    let server = tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let router = router.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), TowerToHyperService::new(router))
                    .await;
            });
        }
    });
    (addr, server, state)
}

async fn dispatch(
    Extract(state): Extract<Shared>,
    method: Method,
    uri: Uri,
    body: Bytes,
) -> Response {
    crate::protocol::handle(&state, &method, uri.path(), &body).await
}

fn server_config(tls_ca: &TestCa) -> rustls::ServerConfig {
    let leaf = tls_ca.issue(&LeafSpec::new(&["127.0.0.1", "localhost"]));
    let chain: Vec<CertificateDer<'static>> =
        CertificateDer::pem_slice_iter(leaf.chain_pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("the test leaf parses");
    let key = PrivateKeyDer::from_pem_slice(leaf.key_pem.as_bytes()).expect("the test key parses");
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("default versions")
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .expect("a usable test certificate")
}
