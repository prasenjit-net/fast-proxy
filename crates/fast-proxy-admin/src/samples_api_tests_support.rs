//! A sample builder shared by the sampling API tests.

use fast_proxy_model::Trigger;
use fast_proxy_sampling::{Sample, SampleRequest, SampleResponse, SampleUpstream};

/// A minimal redacted sample that finished at `at_ms`.
pub(crate) fn sample(at_ms: i64) -> Sample {
    Sample {
        id: None,
        at_ms,
        instance: "a".into(),
        request_id: format!("req-{at_ms}"),
        triggers: vec![Trigger::Slow.as_str().into()],
        primary: Trigger::Slow.as_str().into(),
        subsystem: "shop".into(),
        route: "/".into(),
        client: "203.0.113.9:1".into(),
        request: SampleRequest {
            method: "GET".into(),
            path: "/".into(),
            query: None,
            headers: Vec::new(),
            bytes: 0,
            body: None,
            unusual: Vec::new(),
        },
        response: SampleResponse {
            status: 200,
            headers: Vec::new(),
            bytes: 0,
            body: None,
            body_omitted: None,
        },
        total_ms: 3000,
        upstream: SampleUpstream {
            attempts: Vec::new(),
        },
        suppressed: None,
    }
}
