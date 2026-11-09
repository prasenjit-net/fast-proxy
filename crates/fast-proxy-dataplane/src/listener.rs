use std::net::SocketAddr;

use thiserror::Error;
use tokio::net::TcpListener;

/// One listener to open, as named in the config file.
#[derive(Debug, Clone)]
pub struct ListenerSpec {
    pub name: String,
    pub addr: SocketAddr,
}

/// A listener that is bound and ready to accept.
pub struct BoundListener {
    pub name: String,
    pub(crate) listener: TcpListener,
}

impl BoundListener {
    /// The address actually bound (useful when the spec asked for port 0).
    pub fn local_addr(&self) -> SocketAddr {
        // A freshly bound socket always has an address; the spec's is the fallback.
        self.listener
            .local_addr()
            .unwrap_or_else(|_| ([0, 0, 0, 0], 0).into())
    }
}

/// A listener could not be bound; startup must abort.
#[derive(Debug, Error)]
#[error("cannot bind data plane listener \"{name}\" on {addr}: {source}")]
pub struct BindError {
    pub name: String,
    pub addr: SocketAddr,
    pub source: std::io::Error,
}

/// Binds every listener, or none: the first failure is returned and any
/// listener bound so far is dropped.
pub async fn bind(specs: &[ListenerSpec]) -> Result<Vec<BoundListener>, BindError> {
    let mut bound = Vec::with_capacity(specs.len());
    for spec in specs {
        let listener = TcpListener::bind(spec.addr)
            .await
            .map_err(|source| BindError {
                name: spec.name.clone(),
                addr: spec.addr,
                source,
            })?;
        bound.push(BoundListener {
            name: spec.name.clone(),
            listener,
        });
    }
    Ok(bound)
}
