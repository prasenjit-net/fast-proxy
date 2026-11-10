//! Who the caller is, and the ticket a browser needs to open the WebSocket.

use axum::extract::State;
use axum::Extension;
use axum::Json;
use fast_proxy_auth::{Principal, TicketError};
use serde::Serialize;

use super::envelope::Envelope;
use crate::security::TICKET_LIFETIME_MS;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// The caller as `GET /api/whoami` describes it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhoAmI {
    name: String,
    role: &'static str,
    token_expires_at_ms: Option<i64>,
    /// Whether private keys may be sent over this channel.
    protected_channel: bool,
    /// `required` or `off`.
    auth: &'static str,
}

/// `GET /api/whoami`
pub async fn whoami(
    State(state): State<SharedState>,
    Extension(principal): Extension<Principal>,
) -> Json<Envelope<WhoAmI>> {
    Json(Envelope {
        data: WhoAmI {
            name: principal.name,
            role: principal.role.as_str(),
            token_expires_at_ms: principal.expires_at_ms,
            protected_channel: state.security.channel_protected,
            auth: state.security.auth_mode(),
        },
        version: state.config.snapshot_holder().load().version,
    })
}

/// A WebSocket ticket: single use, short lived.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketDto {
    ticket: String,
    expires_in_secs: i64,
}

/// `POST /api/ws-ticket`: browsers cannot set headers on a WebSocket, so the
/// signed-in page trades its token for a ticket and opens `/ws?ticket=…`.
pub async fn ws_ticket(
    State(state): State<SharedState>,
    Extension(principal): Extension<Principal>,
) -> AppResult<Json<TicketDto>> {
    let ticket = state
        .security
        .tickets
        .issue(principal, crate::clock::now_ms())
        .map_err(|err| match err {
            TicketError::Full => AppError::TooManyRequests {
                retry_after_secs: 5,
            },
            TicketError::Random => AppError::Internal("a ticket could not be created".into()),
        })?;
    Ok(Json(TicketDto {
        ticket,
        expires_in_secs: TICKET_LIFETIME_MS / 1000,
    }))
}
