//! The shared API error type and its JSON envelope.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use fast_proxy_model::FieldError;
use fast_proxy_store::StoreError;
use serde_json::{json, Value};
use thiserror::Error;

/// The one error type every handler returns. Converting it into a
/// response produces a consistent JSON envelope:
///
/// ```json
/// { "error": { "code": "NOT_FOUND", "message": "…", "status": 404 } }
/// ```
///
/// Validation failures add a `fields` array naming each offending input:
///
/// ```json
/// { "error": { "code": "VALIDATION_FAILED", "message": "…", "status": 400,
///              "fields": [{ "field": "stripPrefix", "message": "…" }] } }
/// ```
///
/// The frontend `ApiError` class mirrors this shape, so errors surface
/// as notification bubbles with the same code + message end to end.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    /// The change collides with existing data.
    #[error("{0}")]
    Conflict(String),

    /// The request body exceeds the limit for its endpoint.
    #[error("{0}")]
    TooLarge(String),

    /// The request is understood but refused for who is asking.
    #[error("{0}")]
    Forbidden(String),

    /// One or more inputs are invalid; each names its field.
    #[error("validation failed")]
    Validation(Vec<FieldError>),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) | AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Io(_) | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            AppError::BadRequest(_) => "BAD_REQUEST",
            AppError::NotFound(_) => "NOT_FOUND",
            AppError::Conflict(_) => "CONFLICT",
            AppError::Forbidden(_) => "FORBIDDEN",
            AppError::TooLarge(_) => "PAYLOAD_TOO_LARGE",
            AppError::Validation(_) => "VALIDATION_FAILED",
            AppError::Io(_) => "IO_ERROR",
            AppError::Internal(_) => "INTERNAL",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        if status.is_server_error() {
            tracing::error!(code = self.code(), "request failed: {self}");
        } else {
            tracing::debug!(code = self.code(), "request rejected: {self}");
        }
        // Client-facing messages: BadRequest/NotFound/Internal are safe —
        // their text is always authored by our own handlers. Io wraps a
        // library error via `#[from]`, which can include local file paths
        // or OS error text, so it gets a generic message instead; the real
        // detail already went to tracing above.
        let message = match &self {
            AppError::BadRequest(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::Forbidden(_)
            | AppError::TooLarge(_)
            | AppError::Validation(_)
            | AppError::Internal(_) => self.to_string(),
            AppError::Io(_) => "an internal error occurred".to_string(),
        };
        let mut error = json!({
            "code": self.code(),
            "message": message,
            "status": status.as_u16(),
        });
        if let AppError::Validation(fields) = &self {
            error["fields"] = fields_json(fields);
        }
        let body = json!({ "error": error });
        (status, Json(body)).into_response()
    }
}

/// Field names from the model are snake_case; the API speaks camelCase.
fn fields_json(fields: &[FieldError]) -> Value {
    fields
        .iter()
        .map(|field| json!({ "field": camel_case(&field.field), "message": field.message }))
        .collect()
}

fn camel_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper_next = false;
    for ch in snake.chars() {
        match ch {
            '_' => upper_next = true,
            c if upper_next => {
                out.extend(c.to_uppercase());
                upper_next = false;
            }
            c => out.push(c),
        }
    }
    out
}

impl From<StoreError> for AppError {
    fn from(err: StoreError) -> Self {
        match err {
            StoreError::NotFound(_) => AppError::NotFound(err.to_string()),
            StoreError::Conflict(message) => AppError::Conflict(message),
            StoreError::Invalid(fields) => AppError::Validation(fields),
            // Storage details can include file paths; log them, show nothing.
            StoreError::IncompatibleSchema { .. } | StoreError::Internal(_) => {
                tracing::error!("storage failure: {err}");
                AppError::Internal("an internal error occurred".into())
            }
        }
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;

    async fn body_json(err: AppError) -> serde_json::Value {
        let response = err.into_response();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn bad_request_maps_to_400_with_its_own_message() {
        let body = body_json(AppError::BadRequest("bad title".into())).await;
        assert_eq!(body["error"]["code"], "BAD_REQUEST");
        assert_eq!(body["error"]["status"], 400);
        assert_eq!(body["error"]["message"], "bad title");
    }

    #[tokio::test]
    async fn not_found_maps_to_404_with_its_own_message() {
        let body = body_json(AppError::NotFound("subsystem 1 does not exist".into())).await;
        assert_eq!(body["error"]["code"], "NOT_FOUND");
        assert_eq!(body["error"]["status"], 404);
        assert_eq!(body["error"]["message"], "subsystem 1 does not exist");
    }

    #[tokio::test]
    async fn internal_maps_to_500_with_its_own_message() {
        // AppError::Internal is only ever constructed with text we wrote
        // ourselves, so — unlike Io below — it is safe to show as-is.
        let body = body_json(AppError::Internal("metrics are not available yet".into())).await;
        assert_eq!(body["error"]["code"], "INTERNAL");
        assert_eq!(body["error"]["status"], 500);
        assert_eq!(body["error"]["message"], "metrics are not available yet");
    }

    #[tokio::test]
    async fn io_errors_are_masked_so_local_paths_never_reach_the_client() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "/etc/shadow-ish/secret");
        let body = body_json(AppError::from(io_err)).await;
        assert_eq!(body["error"]["code"], "IO_ERROR");
        assert_eq!(body["error"]["status"], 500);
        let message = body["error"]["message"].as_str().unwrap();
        assert_eq!(message, "an internal error occurred");
        assert!(!message.contains("secret"));
    }

    #[tokio::test]
    async fn conflict_maps_to_409() {
        let body = body_json(AppError::Conflict("name already exists".into())).await;
        assert_eq!(body["error"]["code"], "CONFLICT");
        assert_eq!(body["error"]["status"], 409);
        assert_eq!(body["error"]["message"], "name already exists");
    }

    #[tokio::test]
    async fn validation_lists_camel_cased_fields() {
        let body = body_json(AppError::Validation(vec![
            FieldError::new("upstream_prefix", "bad"),
            FieldError::new("name", "worse"),
        ]))
        .await;
        assert_eq!(body["error"]["code"], "VALIDATION_FAILED");
        assert_eq!(body["error"]["status"], 400);
        assert_eq!(body["error"]["fields"][0]["field"], "upstreamPrefix");
        assert_eq!(body["error"]["fields"][1]["message"], "worse");
    }

    #[test]
    fn camel_case_converts_snake_case() {
        assert_eq!(camel_case("connect_timeout_ms"), "connectTimeoutMs");
        assert_eq!(camel_case("name"), "name");
    }

    #[tokio::test]
    async fn store_errors_map_to_the_matching_api_errors() {
        let not_found = AppError::from(StoreError::NotFound("route 3".into()));
        assert_eq!(body_json(not_found).await["error"]["status"], 404);
        let conflict = AppError::from(StoreError::Conflict("dup".into()));
        assert_eq!(body_json(conflict).await["error"]["status"], 409);
        let invalid = AppError::from(StoreError::invalid_field("addr", "bad"));
        assert_eq!(
            body_json(invalid).await["error"]["fields"][0]["field"],
            "addr"
        );
    }

    #[tokio::test]
    async fn storage_failures_never_leak_their_detail() {
        let body = body_json(AppError::from(StoreError::Internal(
            "/var/db/secret.db".into(),
        )))
        .await;
        assert_eq!(body["error"]["status"], 500);
        assert_eq!(body["error"]["message"], "an internal error occurred");
    }
}
