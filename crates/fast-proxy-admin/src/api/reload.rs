use axum::extract::State;
use axum::Json;

use super::envelope::VersionOnly;
use crate::state::SharedState;
use crate::{AppError, AppResult};

/// `POST /api/reload`: re-read the database now and recompile, for a database
/// edited by hand, an instance that does not poll, or a restore. Answers with
/// the version now live; a stored configuration that does not compile leaves
/// the old one serving and answers 409.
pub async fn reload(State(state): State<SharedState>) -> AppResult<Json<VersionOnly>> {
    match state.config.reload().await {
        Ok(version) => Ok(Json(VersionOnly { version })),
        Err(err) => {
            tracing::warn!("reload failed: {err}");
            Err(AppError::Conflict(
                "the stored configuration could not be loaded; the previous one is still serving \
                 (see the proxy's log)"
                    .into(),
            ))
        }
    }
}
