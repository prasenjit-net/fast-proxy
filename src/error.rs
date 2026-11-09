use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

/// The one error type every handler returns. Converting it into a
/// response produces a consistent JSON envelope:
///
/// ```json
/// { "error": { "code": "NOT_FOUND", "message": "…", "status": 404 } }
/// ```
///
/// The frontend `ApiError` class mirrors this shape, so errors surface
/// as notification bubbles with the same code + message end to end.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Config(_) | AppError::Io(_) | AppError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn code(&self) -> &'static str {
        match self {
            AppError::Config(_) => "CONFIG_ERROR",
            AppError::BadRequest(_) => "BAD_REQUEST",
            AppError::NotFound(_) => "NOT_FOUND",
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
        // their text is always authored by our own handlers. Config and Io
        // wrap a library error via `#[from]`/format!, which can include
        // local file paths or OS error text, so those get a generic
        // message instead; the real detail already went to tracing above.
        let message = match &self {
            AppError::BadRequest(_) | AppError::NotFound(_) | AppError::Internal(_) => {
                self.to_string()
            }
            AppError::Config(_) | AppError::Io(_) => "an internal error occurred".to_string(),
        };
        let body = json!({
            "error": {
                "code": self.code(),
                "message": message,
                "status": status.as_u16(),
            }
        });
        (status, Json(body)).into_response()
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
        let body = body_json(AppError::NotFound("task 1 does not exist".into())).await;
        assert_eq!(body["error"]["code"], "NOT_FOUND");
        assert_eq!(body["error"]["status"], 404);
        assert_eq!(body["error"]["message"], "task 1 does not exist");
    }

    #[tokio::test]
    async fn internal_maps_to_500_with_its_own_message() {
        // AppError::Internal is only ever constructed with text we wrote
        // ourselves, so — unlike Config/Io below — it is safe to show as-is.
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
    async fn config_errors_are_masked_so_file_paths_never_reach_the_client() {
        let body = body_json(AppError::Config(
            "invalid config /home/alice/config.toml".into(),
        ))
        .await;
        assert_eq!(body["error"]["code"], "CONFIG_ERROR");
        let message = body["error"]["message"].as_str().unwrap();
        assert_eq!(message, "an internal error occurred");
        assert!(!message.contains("alice"));
    }
}
