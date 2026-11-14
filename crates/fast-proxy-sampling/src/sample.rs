//! A kept request in its stored form: redacted, capped and ready to be written
//! as JSON. The JSON is the sample's `detail` column; the short columns the
//! database filters on come from the same fields.

use fast_proxy_model::{Trigger, TriggerSet};
use serde::{Deserialize, Serialize};

use crate::raw::{AttemptDetail, CapturedBody, RawSample, Suppressed};
use crate::redact::Redactor;

/// Most headers kept per direction.
pub const MAX_HEADERS: usize = 64;
/// Longest header value kept, in bytes.
pub const MAX_HEADER_VALUE_BYTES: usize = 1024;
/// Longest path or query kept, in bytes.
pub const MAX_PATH_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleBody {
    pub text: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleRequest {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub headers: Vec<(String, String)>,
    pub bytes: u64,
    pub body: Option<SampleBody>,
    /// Why the headers looked unusual, when they did.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unusual: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub bytes: u64,
    pub body: Option<SampleBody>,
    /// Why there is no body: `off`, `budget`, `content_type`, `upgrade`.
    pub body_omitted: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleUpstream {
    pub attempts: Vec<SampleAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleAttempt {
    pub endpoint: String,
    pub failure: Option<String>,
    pub status: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleSuppressed {
    pub count: u32,
    pub since_ms: u32,
}

/// One sample, as stored and as shown by the API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// The database id; absent until the sample has been stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub at_ms: i64,
    pub instance: String,
    pub request_id: String,
    /// Every trigger that matched, in priority order.
    pub triggers: Vec<String>,
    pub primary: String,
    pub subsystem: String,
    pub route: String,
    pub client: String,
    pub request: SampleRequest,
    pub response: SampleResponse,
    pub total_ms: u32,
    pub upstream: SampleUpstream,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<SampleSuppressed>,
}

impl Sample {
    /// Redacts and caps a raw sample.
    pub fn from_raw(raw: RawSample, redactor: &Redactor, instance: &str) -> Self {
        let triggers: Vec<String> = raw
            .triggers
            .iter()
            .map(|t| t.as_str().to_string())
            .collect();
        let primary = raw
            .triggers
            .primary()
            .unwrap_or(Trigger::Baseline)
            .as_str()
            .to_string();
        let query = raw
            .head
            .query
            .as_deref()
            .map(|q| cap(&redactor.query(q), MAX_PATH_BYTES));
        let (request_body, response_body) = (
            raw.request_body
                .as_ref()
                .map(|b| redacted_body(redactor, b)),
            raw.response_body
                .as_ref()
                .map(|b| redacted_body(redactor, b)),
        );
        let body_omitted = [&request_body, &response_body]
            .iter()
            .find_map(|body| body.as_ref().and_then(|(_, omitted)| *omitted))
            .or(raw.body_omitted);
        Self {
            id: None,
            at_ms: raw.at_ms,
            instance: instance.to_string(),
            request_id: raw.request_id,
            triggers,
            primary,
            subsystem: raw.subsystem,
            route: raw.route,
            client: raw.head.client.to_string(),
            request: SampleRequest {
                method: raw.head.method,
                path: cap(&raw.head.path, MAX_PATH_BYTES),
                query,
                headers: redacted_headers(redactor, raw.head.headers),
                bytes: raw.outcome.bytes_in,
                body: request_body.and_then(|(body, _)| body),
                unusual: raw
                    .head
                    .flags
                    .names()
                    .into_iter()
                    .map(String::from)
                    .collect(),
            },
            response: SampleResponse {
                status: raw.outcome.status,
                headers: redacted_headers(redactor, raw.response_headers),
                bytes: raw.outcome.bytes_out,
                body: response_body.and_then(|(body, _)| body),
                body_omitted: body_omitted.map(String::from),
            },
            total_ms: raw.outcome.elapsed_ms,
            upstream: SampleUpstream {
                attempts: raw.attempts.into_iter().map(attempt).collect(),
            },
            suppressed: raw
                .suppressed
                .map(|Suppressed { count, since_ms }| SampleSuppressed { count, since_ms }),
        }
    }

    /// The triggers as a set, for the database's bit mask.
    pub fn trigger_set(&self) -> TriggerSet {
        self.triggers
            .iter()
            .filter_map(|name| Trigger::parse("triggers", name).ok())
            .collect()
    }

    /// The JSON stored in the `detail` column.
    pub fn to_detail_json(&self) -> String {
        // A struct of strings and numbers cannot fail to serialize.
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a sample back from its `detail` JSON.
    pub fn from_detail_json(detail: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(detail)
    }

    /// What this sample counts for against the table's byte limit: its detail
    /// and its short text columns (the store computes the same sum).
    pub fn stored_size(&self, detail: &str) -> usize {
        detail.len()
            + self.instance.len()
            + self.request_id.len()
            + self.subsystem.len()
            + self.route.len()
            + self.client.len()
            + self.request.method.len()
    }
}

fn attempt(detail: AttemptDetail) -> SampleAttempt {
    SampleAttempt {
        endpoint: detail.endpoint,
        failure: detail.failure,
        status: detail.status,
    }
}

fn redacted_headers(redactor: &Redactor, headers: Vec<(String, String)>) -> Vec<(String, String)> {
    headers
        .into_iter()
        .take(MAX_HEADERS)
        .map(|(name, value)| {
            let value = redactor.header_value(&name, &value);
            (name, cap(&value, MAX_HEADER_VALUE_BYTES))
        })
        .collect()
}

/// The redacted body, and the reason when there is none.
fn redacted_body(
    redactor: &Redactor,
    body: &CapturedBody,
) -> (Option<SampleBody>, Option<&'static str>) {
    let out = redactor.body(&body.content_type, &body.bytes, body.truncated);
    match out.omitted {
        Some(reason) => (None, Some(reason)),
        None => (
            Some(SampleBody {
                text: out.text,
                truncated: out.truncated,
            }),
            None,
        ),
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary.
pub fn cap(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}
