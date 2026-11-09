use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::state::SharedState;

/// Append-only access log file. Independent of the file, every request
/// is also logged to the console via `tracing` under the `access` target
/// (filterable with e.g. `--log-level "info,access=warn"`).
pub struct AccessLog {
    file: Option<Mutex<tokio::fs::File>>,
}

impl AccessLog {
    pub async fn open(path: Option<&Path>) -> Self {
        let file = match path {
            Some(path) => match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                Ok(file) => {
                    tracing::info!("access log file: {}", path.display());
                    Some(Mutex::new(file))
                }
                Err(err) => {
                    tracing::warn!("cannot open access log {}: {err}", path.display());
                    None
                }
            },
            None => None,
        };
        Self { file }
    }

    async fn write_line(&self, line: &str) {
        if let Some(file) = &self.file {
            let mut file = file.lock().await;
            let _ = file.write_all(line.as_bytes()).await;
            let _ = file.write_all(b"\n").await;
        }
    }
}

/// Middleware recording one line per request, Common Log Format style:
///
/// `127.0.0.1:52123 - [2026-08-03T10:15:42.123Z] "GET /api/health HTTP/1.1" 200 0.4ms`
pub async fn record(
    State(state): State<SharedState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let started = Instant::now();
    let method = req.method().clone();
    let uri = escape_log_field(&req.uri().to_string());
    let version = req.version();

    let response = next.run(req).await;

    state.requests_total.fetch_add(1, Ordering::Relaxed);
    let status = response.status().as_u16();
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let timestamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ");
    let line =
        format!("{addr} - [{timestamp}] \"{method} {uri} {version:?}\" {status} {elapsed_ms:.1}ms");

    tracing::info!(target: "access", "{line}");
    state.access_log.write_line(&line).await;

    response
}

/// Escapes `"` and control characters in an attacker-controlled field
/// (here, the request URI) before it goes into the quoted CLF-style log
/// line — otherwise a crafted path could inject a literal quote or a
/// newline and forge extra log fields/lines.
fn escape_log_field(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_log_field;

    #[test]
    fn passes_through_ordinary_paths() {
        assert_eq!(escape_log_field("/api/tasks?x=1"), "/api/tasks?x=1");
    }

    #[test]
    fn escapes_quotes_so_the_field_cannot_be_broken_out_of() {
        assert_eq!(escape_log_field("/x\"injected"), "/x\\\"injected");
    }

    #[test]
    fn escapes_control_characters_so_lines_cannot_be_forged() {
        assert_eq!(escape_log_field("/x\r\nFAKE LINE"), "/x\\x0d\\x0aFAKE LINE");
    }

    #[test]
    fn escapes_backslash_itself() {
        assert_eq!(escape_log_field("/x\\y"), "/x\\\\y");
    }
}
