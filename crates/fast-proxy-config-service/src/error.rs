use fast_proxy_routing::CompileError;
use fast_proxy_store::StoreError;
use thiserror::Error;

/// Why the service could not start from the stored configuration.
#[derive(Debug, Error)]
pub enum StartError {
    #[error("cannot read the stored configuration: {0}")]
    Store(#[from] StoreError),
    #[error("the stored configuration does not compile: {0}")]
    Compile(#[from] CompileError),
}
