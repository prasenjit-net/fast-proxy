//! Admin tokens: create (the secret appears only in that response), list and
//! revoke. All of it is `tokens` access: admin only.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_model::{Role, TokenId, TokenName};
use fast_proxy_store::repository::tokens::TokenRow;
use serde::{Deserialize, Serialize};

use super::change::Change;
use super::envelope::{Envelope, VersionOnly};
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// Longest token lifetime accepted: ten years.
const MAX_EXPIRY_DAYS: u32 = 3_650;
const DAY_MS: i64 = 86_400_000;

/// Body of `POST /api/tokens`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTokenRequest {
    name: String,
    role: String,
    /// Absent means the token never expires.
    expires_in_days: Option<u32>,
}

/// A token as listed: never its secret or hash.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenDto {
    id: i64,
    name: String,
    role: &'static str,
    created_at_ms: i64,
    created_by: String,
    expires_at_ms: Option<i64>,
    revoked_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
}

/// The response to creating a token: the only time its secret is shown.
#[derive(Debug, Serialize)]
pub struct CreatedTokenDto {
    #[serde(flatten)]
    token: TokenDto,
    /// The complete `fp_<id>.<secret>`; store it now, it cannot be shown again.
    secret: String,
}

fn token_dto(row: TokenRow) -> TokenDto {
    TokenDto {
        id: row.id.0,
        name: row.name.as_str().to_string(),
        role: row.role.as_str(),
        created_at_ms: row.created_at_ms,
        created_by: row.created_by,
        expires_at_ms: row.expires_at_ms,
        revoked_at_ms: row.revoked_at_ms,
        last_used_at_ms: row.last_used_at_ms,
    }
}

/// `POST /api/tokens`
pub async fn create_token(
    State(state): State<SharedState>,
    Change(context): Change,
    ApiJson(body): ApiJson<CreateTokenRequest>,
) -> AppResult<(StatusCode, Json<Envelope<CreatedTokenDto>>)> {
    let mut errors = FieldErrors::default();
    let name = errors.check(TokenName::parse(&body.name));
    let role = errors.check(Role::parse(&body.role));
    let days = body.expires_in_days;
    if days.is_some_and(|days| days == 0 || days > MAX_EXPIRY_DAYS) {
        errors.add("expiresInDays", "must be between 1 and 3650");
    }
    errors.finish()?;
    let (Some(name), Some(role)) = (name, role) else {
        unreachable!("finish() fails when a field is invalid");
    };
    let expires_at_ms = days.map(|days| crate::clock::now_ms() + i64::from(days) * DAY_MS);

    let applied = state
        .config
        .create_token(&context, name.as_str(), role, expires_at_ms)
        .await?;
    let created = applied.value;
    let row = state
        .config
        .list_tokens()
        .await?
        .into_iter()
        .find(|row| row.id == created.id)
        .ok_or_else(|| AppError::Internal("the new token cannot be read back".into()))?;
    state.token_changed("created", name.as_str());
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data: CreatedTokenDto {
                token: token_dto(row),
                secret: created.token.reveal().to_string(),
            },
            version: applied.version,
        }),
    ))
}

/// `GET /api/tokens`
pub async fn list_tokens(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<Vec<TokenDto>>>> {
    let tokens = state.config.list_tokens().await?;
    Ok(Json(Envelope {
        data: tokens.into_iter().map(token_dto).collect(),
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `DELETE /api/tokens/{id}`: revokes at once; the last usable admin token is a 409.
pub async fn revoke_token(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    let id = TokenId(id);
    let name = state
        .config
        .list_tokens()
        .await?
        .into_iter()
        .find(|row| row.id == id)
        .map(|row| row.name.as_str().to_string());
    let applied = state.config.revoke_token(&context, id).await?;
    if let Some(name) = name {
        state.token_changed("revoked", &name);
    }
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
