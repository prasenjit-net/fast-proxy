use axum::extract::State;
use axum::Json;
use serde::Serialize;

use super::envelope::Envelope;
use crate::state::SharedState;

/// The process-wide TLS state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsStatus {
    enabled: bool,
    listen: Option<String>,
    http2: bool,
    min_version: String,
    /// `ok`, `key mismatch` or `not created`.
    vault: &'static str,
}

/// `GET /api/tls`: whether TLS is on, how, and whether stored keys can be read.
pub async fn tls_status(State(state): State<SharedState>) -> Json<Envelope<TlsStatus>> {
    let tls = &state.runtime.tls;
    Json(Envelope {
        data: TlsStatus {
            enabled: tls.enabled,
            listen: tls.listen.map(|addr| addr.to_string()),
            http2: tls.http2,
            min_version: tls.min_version.clone(),
            vault: state.config.vault_status().as_str(),
        },
        version: state.config.snapshot_holder().load().version,
    })
}
