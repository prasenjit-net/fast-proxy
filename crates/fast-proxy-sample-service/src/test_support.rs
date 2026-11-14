//! Builders shared by the service's tests.

use std::path::PathBuf;
use std::time::Duration;

use fast_proxy_model::Trigger;
use fast_proxy_sampling::{Sample, SampleRequest, SampleResponse, SampleUpstream};
use fast_proxy_store::Database;
use tempfile::TempDir;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);

/// A database file on disk, so several connections (and threads) can share it.
pub(crate) struct Shared {
    pub(crate) _dir: TempDir,
    pub(crate) path: PathBuf,
    /// The configuration side's connection, which also created the schema.
    pub(crate) primary: Database,
}

impl Shared {
    pub(crate) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared.db");
        let primary = Database::open(&path).unwrap();
        Self {
            _dir: dir,
            path,
            primary,
        }
    }

    pub(crate) fn secondary(&self) -> Database {
        Database::open_secondary(&self.path, TIMEOUT).unwrap()
    }
}

/// A redacted sample that finished at `at_ms` on `instance`.
pub(crate) fn sample(n: i64, instance: &str, subsystem: &str) -> Sample {
    Sample {
        id: None,
        at_ms: n,
        instance: instance.into(),
        request_id: format!("req-{instance}-{n}"),
        triggers: vec![Trigger::UpstreamFailure.as_str().into()],
        primary: Trigger::UpstreamFailure.as_str().into(),
        subsystem: subsystem.into(),
        route: "/api".into(),
        client: "203.0.113.9:5000".into(),
        request: SampleRequest {
            method: "GET".into(),
            path: "/api/orders".into(),
            query: None,
            headers: vec![("accept".into(), "*/*".into())],
            bytes: 0,
            body: None,
            unusual: Vec::new(),
        },
        response: SampleResponse {
            status: 502,
            headers: Vec::new(),
            bytes: 0,
            body: None,
            body_omitted: None,
        },
        total_ms: 30,
        upstream: SampleUpstream {
            attempts: Vec::new(),
        },
        suppressed: None,
    }
}
