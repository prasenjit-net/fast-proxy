use axum::body::Body;
use axum::extract::State;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::error::AppError;
use crate::state::SharedState;

/// The compiled Vite app, embedded into the binary at build time.
/// In debug builds rust-embed reads ui/dist from disk instead, so a
/// fresh `npm run build` shows up without recompiling the server.
#[derive(RustEmbed)]
#[folder = "ui/dist"]
struct Assets;

pub async fn handler(State(state): State<SharedState>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    // Unknown /api routes get the JSON 404 envelope, not the SPA shell.
    if path.starts_with("api/") {
        return AppError::NotFound(format!("no such endpoint: /{path}")).into_response();
    }

    if !path.is_empty() && path != "index.html" {
        if let Some(asset) = Assets::get(path) {
            return serve(path, asset);
        }
    }

    // SPA fallback: any other route serves index.html and the client
    // router takes over (this is what makes deep links work).
    match Assets::get("index.html") {
        Some(asset) => serve_index(&state, asset),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "UI not built. Run `npm install && npm run build` inside ui/, then restart (debug) or rebuild (release) the server.",
        )
            .into_response(),
    }
}

/// index.html carries a `<meta name="default-theme" content="…">`
/// placeholder (see ui/index.html) so the pre-React boot script can apply
/// [ui].default_theme on first paint instead of only guessing from the
/// OS preference — otherwise first-time visitors see a flash from the
/// OS-guessed theme to the real default once the SPA loads and fetches
/// /api/config.
fn serve_index(state: &SharedState, asset: rust_embed::EmbeddedFile) -> Response {
    let html = String::from_utf8_lossy(&asset.data);
    let theme = html_attr_escape(&state.config.ui.default_theme);
    let html = html.replace("__DEFAULT_THEME__", &theme);
    (
        [
            (
                header::CONTENT_TYPE,
                mime_guess::mime::TEXT_HTML_UTF_8.to_string(),
            ),
            (header::CACHE_CONTROL, "no-cache".to_string()),
        ],
        html,
    )
        .into_response()
}

fn html_attr_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn serve(path: &str, asset: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    // Vite fingerprints everything under assets/, so those can be cached
    // forever; everything else must always be revalidated.
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let body = match asset.data {
        std::borrow::Cow::Borrowed(bytes) => Body::from(bytes),
        std::borrow::Cow::Owned(bytes) => Body::from(bytes),
    };
    (
        [
            (header::CONTENT_TYPE, mime.as_ref().to_string()),
            (header::CACHE_CONTROL, cache.to_string()),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::html_attr_escape;

    #[test]
    fn escapes_html_special_characters() {
        assert_eq!(
            html_attr_escape(r#"<script>&"x"#),
            "&lt;script&gt;&amp;&quot;x"
        );
    }

    #[test]
    fn passes_through_ordinary_values() {
        assert_eq!(html_attr_escape("auto"), "auto");
        assert_eq!(html_attr_escape("dark"), "dark");
    }
}
