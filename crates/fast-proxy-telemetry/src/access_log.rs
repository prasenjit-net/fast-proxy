use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

/// Which plane served the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plane {
    Admin,
    Data,
}

impl Plane {
    fn label(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Data => "data",
        }
    }
}

/// What only a proxied (data plane) request has.
#[derive(Debug, Clone, PartialEq)]
pub struct DataPlaneFields {
    /// The matched subsystem's name, or `_unmatched`.
    pub subsystem: String,
    /// The matched route's prefix, or `_none`.
    pub route: String,
    /// The upstream endpoint, when the request was forwarded.
    pub upstream: Option<String>,
    /// How many upstream attempts were made (0 when nothing was forwarded).
    pub attempts: u32,
    pub request_id: String,
    /// Time until the upstream's response head arrived, when forwarded.
    pub upstream_time: Option<Duration>,
    /// Response body bytes sent to the client.
    pub bytes_out: u64,
}

/// One finished request, as written to the access log.
pub struct AccessRecord<'a> {
    pub plane: Plane,
    pub client: SocketAddr,
    pub method: &'a str,
    pub uri: &'a str,
    pub version: &'a str,
    pub status: u16,
    /// Time from receiving the request to finishing the response.
    pub elapsed: Duration,
    pub data_plane: Option<&'a DataPlaneFields>,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

impl AccessRecord<'_> {
    /// Renders one line: a Common-Log-style request part followed by
    /// `key=value` fields.
    ///
    /// `127.0.0.1:52123 - [2026-08-03T10:15:42.123Z] "GET /api/health HTTP/1.1" 200 plane=admin total_ms=0.4`
    ///
    /// Data plane lines add `subsystem`, `route`, `upstream` (the endpoint
    /// that served the request, or the last one tried), `attempts`,
    /// `request_id`, `upstream_ms`, and `bytes_out`; fields that do not
    /// apply are `-`.
    fn format_line(&self, at: DateTime<Utc>) -> String {
        let timestamp = at.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let uri = escape_log_field(self.uri);
        let mut line = format!(
            "{} - [{timestamp}] \"{} {uri} {}\" {} plane={}",
            self.client,
            self.method,
            self.version,
            self.status,
            self.plane.label()
        );
        if let Some(data) = self.data_plane {
            let upstream = data.upstream.as_deref().unwrap_or("-");
            let upstream_ms = data
                .upstream_time
                .map_or_else(|| "-".to_string(), |time| format!("{:.1}", ms(time)));
            line.push_str(&format!(
                " subsystem={} route={} upstream={} attempts={} request_id={} upstream_ms={upstream_ms} bytes_out={}",
                escape_log_field(&data.subsystem),
                escape_log_field(&data.route),
                escape_log_field(upstream),
                data.attempts,
                escape_log_field(&data.request_id),
                data.bytes_out
            ));
        }
        line.push_str(&format!(" total_ms={:.1}", ms(self.elapsed)));
        line
    }
}

enum Message {
    Line(String),
    Flush(oneshot::Sender<()>),
}

/// Append-only access log. Independent of the file, every request is also
/// logged to the console via `tracing` under the `access` target
/// (filterable with e.g. `--log-level "info,access=warn"`).
///
/// Writing to the file happens on a background task, so recording a request
/// never waits on the disk.
pub struct AccessLog {
    writer: Option<mpsc::UnboundedSender<Message>>,
}

impl AccessLog {
    /// Opens the log file for appending. A missing path or an unopenable
    /// file disables the file sink (with a warning) but never the console line.
    pub async fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self { writer: None };
        };
        let opened = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await;
        match opened {
            Ok(file) => {
                tracing::info!("access log file: {}", path.display());
                let (sender, inbox) = mpsc::unbounded_channel();
                tokio::spawn(write_lines(file, inbox));
                Self {
                    writer: Some(sender),
                }
            }
            Err(err) => {
                tracing::warn!("cannot open access log {}: {err}", path.display());
                Self { writer: None }
            }
        }
    }

    /// Writes one record to the console and queues it for the file.
    pub fn record(&self, record: &AccessRecord<'_>) {
        let line = record.format_line(Utc::now());
        tracing::info!(target: "access", "{line}");
        if let Some(writer) = &self.writer {
            // The writer only stops when the log is dropped.
            let _ = writer.send(Message::Line(line));
        }
    }

    /// Resolves once everything recorded so far has been written to the file.
    pub async fn flush(&self) {
        let Some(writer) = &self.writer else {
            return;
        };
        let (done, written) = oneshot::channel();
        if writer.send(Message::Flush(done)).is_ok() {
            let _ = written.await;
        }
    }
}

async fn write_lines(mut file: tokio::fs::File, mut inbox: mpsc::UnboundedReceiver<Message>) {
    while let Some(message) = inbox.recv().await {
        match message {
            Message::Line(line) => {
                // A failing log disk must not take the proxy down.
                let _ = file.write_all(line.as_bytes()).await;
                let _ = file.write_all(b"\n").await;
            }
            Message::Flush(done) => {
                let _ = file.flush().await;
                let _ = done.send(());
            }
        }
    }
}

/// Escapes `"` and control characters in an attacker-controlled field
/// (the request URI, a client-supplied request id) before it goes into
/// the log line — otherwise a crafted value could inject a literal quote or
/// a newline and forge extra log fields/lines.
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

    fn admin_record(uri: &str) -> AccessRecord<'_> {
        AccessRecord {
            plane: Plane::Admin,
            client: SocketAddr::from(([127, 0, 0, 1], 52123)),
            method: "GET",
            uri,
            version: "HTTP/1.1",
            status: 200,
            elapsed: Duration::from_micros(400),
            data_plane: None,
        }
    }

    fn data_fields() -> DataPlaneFields {
        DataPlaneFields {
            subsystem: "shop".into(),
            route: "/api".into(),
            upstream: Some("127.0.0.1:3000".into()),
            attempts: 1,
            request_id: "abc123".into(),
            upstream_time: Some(Duration::from_micros(1200)),
            bytes_out: 456,
        }
    }

    fn at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 8, 3, 10, 15, 42).unwrap()
    }

    /// Splits the key=value tail of a line into a lookup.
    fn fields_of(line: &str) -> std::collections::HashMap<&str, &str> {
        line.split(' ')
            .filter_map(|part| part.split_once('='))
            .collect()
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
    fn admin_lines_carry_the_plane_and_total_time() {
        assert_eq!(
            admin_record("/api/health").format_line(at()),
            "127.0.0.1:52123 - [2026-08-03T10:15:42.000Z] \"GET /api/health HTTP/1.1\" 200 plane=admin total_ms=0.4"
        );
    }

    #[test]
    fn data_plane_lines_carry_every_field() {
        let fields = data_fields();
        let record = AccessRecord {
            plane: Plane::Data,
            data_plane: Some(&fields),
            ..admin_record("/api/users?page=2")
        };
        let line = record.format_line(at());
        assert_eq!(
            line,
            "127.0.0.1:52123 - [2026-08-03T10:15:42.000Z] \"GET /api/users?page=2 HTTP/1.1\" 200 \
             plane=data subsystem=shop route=/api upstream=127.0.0.1:3000 attempts=1 request_id=abc123 \
             upstream_ms=1.2 bytes_out=456 total_ms=0.4"
        );
    }

    #[test]
    fn fields_that_do_not_apply_are_dashes() {
        let fields = DataPlaneFields {
            subsystem: "_unmatched".into(),
            route: "_none".into(),
            upstream: None,
            attempts: 0,
            upstream_time: None,
            ..data_fields()
        };
        let record = AccessRecord {
            plane: Plane::Data,
            status: 404,
            data_plane: Some(&fields),
            ..admin_record("/")
        };
        let line = record.format_line(at());
        let parsed = fields_of(&line);
        assert_eq!(parsed["upstream"], "-");
        assert_eq!(parsed["attempts"], "0");
        assert_eq!(parsed["upstream_ms"], "-");
        assert_eq!(parsed["subsystem"], "_unmatched");
        assert_eq!(parsed["route"], "_none");
    }

    #[test]
    fn a_hostile_request_id_cannot_forge_extra_fields_or_lines() {
        let fields = DataPlaneFields {
            request_id: "x\nplane=admin".into(),
            ..data_fields()
        };
        let record = AccessRecord {
            plane: Plane::Data,
            data_plane: Some(&fields),
            ..admin_record("/")
        };
        let line = record.format_line(at());
        assert!(!line.contains('\n'));
        // The id stays one escaped token; no separate `plane=admin` field appears.
        assert_eq!(fields_of(&line)["request_id"], "x\\x0aplane=admin");
        let separate_plane_fields = line.split(' ').filter(|p| p.starts_with("plane=")).count();
        assert_eq!(separate_plane_fields, 1);
    }

    #[tokio::test]
    async fn records_are_appended_to_the_file_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("access.log");
        let log = AccessLog::open(Some(&path)).await;

        log.record(&admin_record("/one"));
        log.record(&admin_record("/two"));
        log.flush().await;

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
        log.record(&admin_record("/x"));
        log.flush().await;
        assert!(!missing_parent.exists());
    }

    #[tokio::test]
    async fn flushing_without_a_file_returns_immediately() {
        AccessLog::open(None).await.flush().await;
    }
}
