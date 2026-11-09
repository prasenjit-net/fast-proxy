use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// One finished request, as written to the access log.
pub struct AccessRecord<'a> {
    pub client: SocketAddr,
    pub method: &'a str,
    pub uri: &'a str,
    pub version: &'a str,
    pub status: u16,
    pub elapsed: Duration,
}

impl AccessRecord<'_> {
    /// Renders the Common-Log-style line:
    ///
    /// `127.0.0.1:52123 - [2026-08-03T10:15:42.123Z] "GET /api/health HTTP/1.1" 200 0.4ms`
    fn format_line(&self, at: DateTime<Utc>) -> String {
        let timestamp = at.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let uri = escape_log_field(self.uri);
        let elapsed_ms = self.elapsed.as_secs_f64() * 1000.0;
        format!(
            "{} - [{timestamp}] \"{} {uri} {}\" {} {elapsed_ms:.1}ms",
            self.client, self.method, self.version, self.status
        )
    }
}

/// Append-only access log file. Independent of the file, every request
/// is also logged to the console via `tracing` under the `access` target
/// (filterable with e.g. `--log-level "info,access=warn"`).
pub struct AccessLog {
    file: Option<Mutex<tokio::fs::File>>,
}

impl AccessLog {
    /// Opens the log file for appending. A missing path or an unopenable
    /// file disables the file sink (with a warning) but never the console line.
    pub async fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self { file: None };
        };
        let opened = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await;
        match opened {
            Ok(file) => {
                tracing::info!("access log file: {}", path.display());
                Self {
                    file: Some(Mutex::new(file)),
                }
            }
            Err(err) => {
                tracing::warn!("cannot open access log {}: {err}", path.display());
                Self { file: None }
            }
        }
    }

    /// Writes one record to the console and, if configured, the file.
    pub async fn record(&self, record: &AccessRecord<'_>) {
        let line = record.format_line(Utc::now());
        tracing::info!(target: "access", "{line}");
        self.write_line(&line).await;
    }

    async fn write_line(&self, line: &str) {
        let Some(file) = &self.file else {
            return;
        };
        let mut file = file.lock().await;
        // A failing log disk must not fail the request being logged.
        let _ = file.write_all(line.as_bytes()).await;
        let _ = file.write_all(b"\n").await;
    }
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
    use chrono::TimeZone;

    use super::*;

    fn record(uri: &str) -> AccessRecord<'_> {
        AccessRecord {
            client: SocketAddr::from(([127, 0, 0, 1], 52123)),
            method: "GET",
            uri,
            version: "HTTP/1.1",
            status: 200,
            elapsed: Duration::from_micros(400),
        }
    }

    #[test]
    fn passes_through_ordinary_paths() {
        assert_eq!(escape_log_field("/api/x?x=1"), "/api/x?x=1");
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

    #[test]
    fn formats_a_common_log_style_line() {
        let at = Utc.with_ymd_and_hms(2026, 8, 3, 10, 15, 42).unwrap();
        assert_eq!(
            record("/api/health").format_line(at),
            "127.0.0.1:52123 - [2026-08-03T10:15:42.000Z] \"GET /api/health HTTP/1.1\" 200 0.4ms"
        );
    }

    #[tokio::test]
    async fn records_are_appended_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(Some(&path)).await;

        log.record(&record("/one")).await;
        log.record(&record("/two")).await;

        let written = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"GET /one HTTP/1.1\" 200"));
        assert!(lines[1].contains("\"GET /two HTTP/1.1\" 200"));
    }

    #[tokio::test]
    async fn an_unopenable_path_disables_the_file_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        let missing_parent = dir.path().join("no-such-dir").join("access.log");
        let log = AccessLog::open(Some(&missing_parent)).await;
        log.record(&record("/x")).await;
        assert!(!missing_parent.exists());
    }
}
