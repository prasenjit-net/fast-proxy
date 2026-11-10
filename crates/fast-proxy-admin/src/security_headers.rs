//! Response headers that keep a browser from misusing the admin plane.

use axum::extract::Request;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, REFERRER_POLICY, X_CONTENT_TYPE_OPTIONS,
    X_FRAME_OPTIONS,
};
use axum::http::HeaderValue;
use axum::middleware::Next;
use axum::response::Response;

/// Scripts and styles only from the admin origin, no framing, no plugins. The
/// UI ships its script as a file and holds the bearer token in
/// `sessionStorage`, which this policy is what makes acceptable.
const CONTENT_SECURITY_POLICY_VALUE: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self' ws: wss:; \
     object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// Adds the security headers to every response, and `no-store` to the API and
/// metrics (answers that must never sit in a cache).
pub async fn add(request: Request, next: Next) -> Response {
    let sensitive = is_api_path(request.uri().path());
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY_VALUE),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    if sensitive {
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn is_api_path(path: &str) -> bool {
    path.starts_with("/api/") || path == "/metrics" || path == "/ws"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_and_metrics_are_not_cacheable_but_the_ui_is() {
        for path in ["/api/status", "/api/health", "/metrics", "/ws"] {
            assert!(is_api_path(path), "{path}");
        }
        for path in ["/", "/assets/app.js", "/subsystems/1", "/apidocs"] {
            assert!(!is_api_path(path), "{path}");
        }
    }

    #[test]
    fn the_policy_forbids_inline_script_and_foreign_origins() {
        assert!(CONTENT_SECURITY_POLICY_VALUE.contains("script-src 'self';"));
        assert!(!CONTENT_SECURITY_POLICY_VALUE.contains("script-src 'self' 'unsafe"));
        assert!(CONTENT_SECURITY_POLICY_VALUE.contains("frame-ancestors 'none'"));
        assert!(!CONTENT_SECURITY_POLICY_VALUE.contains("http:"));
    }
}
