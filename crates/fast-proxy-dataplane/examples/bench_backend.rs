//! A minimal, fast HTTP/1.1 backend for benchmarking the proxy: every request
//! gets the same fixed response. Not part of the product.
//!
//! ```text
//! cargo run --release -p fast-proxy-dataplane --example bench_backend -- 127.0.0.1:3001 [body_bytes]
//! ```

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http::Response;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let addr: SocketAddr = args
        .next()
        .ok_or("usage: bench_backend ADDR [BODY_BYTES]")?
        .parse()?;
    let body_bytes: usize = args.next().map_or(Ok(2), |raw| raw.parse())?;
    let body = Bytes::from(vec![b'x'; body_bytes]);

    let listener = TcpListener::bind(addr).await?;
    eprintln!("bench backend on http://{addr}, {body_bytes}-byte responses");
    loop {
        let (stream, _) = listener.accept().await?;
        let body = body.clone();
        tokio::spawn(async move {
            let service = service_fn(move |_request| {
                let body = body.clone();
                async move { Ok::<_, Infallible>(Response::new(Full::new(body))) }
            });
            let _ = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await;
        });
    }
}
