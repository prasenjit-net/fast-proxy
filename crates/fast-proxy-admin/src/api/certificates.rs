use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::Json;
use fast_proxy_config_service::CertificateChange;
use fast_proxy_model::{CertificateId, SubsystemId};
use serde::Deserialize;

use super::certificate_dto::{now_ms, CertificateDto};
use super::change::Change;
use super::envelope::{Envelope, VersionOnly};
use super::json::ApiJson;
use super::key_safety::require_loopback_for_keys;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// Body of `POST /api/certificates` and `POST /api/subsystems/{id}/certificate`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadRequest {
    name: String,
    cert_pem: String,
    key_pem: String,
}

/// Body of `PUT /api/certificates/{id}`; absent fields keep the stored value.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplaceRequest {
    name: Option<String>,
    cert_pem: Option<String>,
    key_pem: Option<String>,
}

/// Body of `POST /api/certificates/self-signed`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelfSignedRequest {
    name: String,
    names: Vec<String>,
}

/// Finds one certificate in the listing and shapes it for a response.
async fn certificate_dto(
    state: &SharedState,
    id: CertificateId,
    warnings: Vec<String>,
) -> AppResult<CertificateDto> {
    let listed = state.config.list_certificates().await?;
    let summary = listed
        .iter()
        .find(|summary| summary.id == id)
        .ok_or_else(|| AppError::NotFound(format!("certificate {} not found", id.0)))?;
    Ok(CertificateDto::new(summary, now_ms(), warnings))
}

/// `GET /api/certificates`: metadata only.
pub async fn list_certificates(
    State(state): State<SharedState>,
) -> AppResult<Json<Envelope<Vec<CertificateDto>>>> {
    let listed = state.config.list_certificates().await?;
    let now = now_ms();
    Ok(Json(Envelope {
        data: listed
            .iter()
            .map(|summary| CertificateDto::new(summary, now, Vec::new()))
            .collect(),
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `POST /api/certificates` (loopback only: the body carries a private key).
pub async fn upload_certificate(
    State(state): State<SharedState>,
    Change(context): Change,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    ApiJson(body): ApiJson<UploadRequest>,
) -> AppResult<(StatusCode, Json<Envelope<CertificateDto>>)> {
    require_loopback_for_keys(peer)?;
    let applied = state
        .config
        .upload_certificate(&context, &body.name, &body.cert_pem, &body.key_pem)
        .await?;
    state.certificate_changed("uploaded", &body.name);
    let data = certificate_dto(&state, applied.value.id, applied.value.warnings).await?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data,
            version: applied.version,
        }),
    ))
}

/// `POST /api/certificates/self-signed` (loopback only, like every key-writing call).
pub async fn generate_self_signed(
    State(state): State<SharedState>,
    Change(context): Change,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    ApiJson(body): ApiJson<SelfSignedRequest>,
) -> AppResult<(StatusCode, Json<Envelope<CertificateDto>>)> {
    require_loopback_for_keys(peer)?;
    let applied = state
        .config
        .generate_self_signed(&context, &body.name, body.names)
        .await?;
    state.certificate_changed("generated", &body.name);
    let data = certificate_dto(&state, applied.value, Vec::new()).await?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data,
            version: applied.version,
        }),
    ))
}

/// `PUT /api/certificates/{id}`: rename and/or replace the chain and key. A
/// body that carries a key is loopback only.
pub async fn replace_certificate(
    State(state): State<SharedState>,
    Change(context): Change,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<ReplaceRequest>,
) -> AppResult<Json<Envelope<CertificateDto>>> {
    if body.key_pem.is_some() {
        require_loopback_for_keys(peer)?;
    }
    let id = CertificateId(id);
    let applied = state
        .config
        .replace_certificate(
            &context,
            id,
            CertificateChange {
                name: body.name,
                cert_pem: body.cert_pem,
                key_pem: body.key_pem,
            },
        )
        .await?;
    let data = certificate_dto(&state, id, applied.value.warnings).await?;
    state.certificate_changed("replaced", &data.name);
    Ok(Json(Envelope {
        data,
        version: applied.version,
    }))
}

/// `DELETE /api/certificates/{id}`: 409 while a subsystem uses it.
pub async fn delete_certificate(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    let id = CertificateId(id);
    let name = state
        .config
        .list_certificates()
        .await?
        .into_iter()
        .find(|summary| summary.id == id)
        .map(|summary| summary.name.as_str().to_string())
        .ok_or_else(|| AppError::NotFound(format!("certificate {} not found", id.0)))?;
    let applied = state.config.delete_certificate(&context, id).await?;
    state.certificate_changed("deleted", &name);
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}

/// `POST /api/subsystems/{id}/certificate`: upload and attach in one step
/// (loopback only).
pub async fn upload_subsystem_certificate(
    State(state): State<SharedState>,
    Change(context): Change,
    ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>,
    Path(id): Path<i64>,
    ApiJson(body): ApiJson<UploadRequest>,
) -> AppResult<(StatusCode, Json<Envelope<CertificateDto>>)> {
    require_loopback_for_keys(peer)?;
    let applied = state
        .config
        .upload_certificate_for_subsystem(
            &context,
            SubsystemId(id),
            &body.name,
            &body.cert_pem,
            &body.key_pem,
        )
        .await?;
    let (outcome, _uncovered) = applied.value;
    state.certificate_changed("uploaded", &body.name);
    let data = certificate_dto(&state, outcome.id, outcome.warnings).await?;
    Ok((
        StatusCode::CREATED,
        Json(Envelope {
            data,
            version: applied.version,
        }),
    ))
}

/// `DELETE /api/subsystems/{id}/certificate`: detach. While TLS is on and HTTPS
/// is enabled a development certificate takes over.
pub async fn detach_subsystem_certificate(
    State(state): State<SharedState>,
    Change(context): Change,
    Path(id): Path<i64>,
) -> AppResult<Json<VersionOnly>> {
    let id = SubsystemId(id);
    let current = state
        .config
        .read(move |conn| fast_proxy_store::repository::subsystems::get(conn, id))
        .await?
        .value;
    let https = fast_proxy_model::HttpsPolicy {
        certificate_id: None,
        ..current.https
    };
    let applied = state
        .config
        .set_subsystem_https(&context, id, https)
        .await?;
    Ok(Json(VersionOnly {
        version: applied.version,
    }))
}
