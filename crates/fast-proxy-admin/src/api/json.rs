use axum::extract::{FromRequest, Request};
use axum::Json;
use serde::de::DeserializeOwned;

use crate::AppError;

/// Like [`Json`], but malformed bodies are rejected with the standard
/// JSON error envelope instead of plain text.
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request(request: Request, state: &S) -> Result<Self, AppError> {
        match Json::<T>::from_request(request, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) if rejection.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE => {
                Err(AppError::TooLarge("the request body is too large".into()))
            }
            Err(rejection) => Err(AppError::BadRequest(rejection.body_text())),
        }
    }
}
