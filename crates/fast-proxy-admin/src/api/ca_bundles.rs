use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::change::Change;
use super::envelope::{Envelope, VersionOnly};
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;

/// Body of `POST /api/ca-bundles`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaBundleRequest {
    name: String,
    pem: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaCertificateDto {
    subject: String,
    not_after_ms: i64,
}

/// A CA bundle: public material, listed by name and certificate.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaBundleDto {
    id: i64,
    name: String,
    cert_count: usize,
    certificates: Vec<CaCertificateDto>,
    updated_at_ms: i64,
}

async fn bundle_dtos(state: &SharedState) -> AppResult<Vec<CaBundleDto>> {
    Ok(state
        .config
        .list_ca_bundles()
        .await?
        .into_iter()
        .map(|bundle| CaBundleDto {
            id: bundle.id,
            name: bundle.name.as_str().to_string(),
            cert_count: bundle.certificates.len(),
            certificates: bundle
                .certificates
                .into_iter()
                .map(|cert| CaCertificateDto {
                    subject: cert.subject,
                    not_after_ms: cert.not_after_ms,
                })
                .collect(),
            updated_at_ms: bundle.updated_at_ms,
        })
        .collect())
}

/// `GET /api/ca-bundles`
pub async fn list_ca_bundles(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<Vec<CaBundleDto>>>> {
    Ok(Json(Envelope {
        data: bundle_dtos(&state).await?,
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `POST /api/ca-bundles`
pub async fn upload_ca_bundle(
    State(state): State<SharedState>,
    Change(context): Change,
    ApiJson(body): ApiJson<CaBundleRequest>,
) -> AppResult<(StatusCode, Json<Envelope<CaBundleDto>>)> {
    let applied = state
        .config
        .upload_ca_bundle(&context, &body.name, &body.pem)
        .await?;
    let dto = bundle_dtos(&state)
        .await?
        .into_iter()
        .find(|bundle| bundle.id == applied.value);
    let data = dto.ok_or_else(|| crate::AppError::Internal("a stored bundle vanished".into()))?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data,
            version: applied.version,
        }),
    ))
}

/// `DELETE /api/ca-bundles/{id}`: 409 while an upstream names the bundle.
pub async fn delete_ca_bundle(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    let applied = state.config.delete_ca_bundle(&context, id).await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
