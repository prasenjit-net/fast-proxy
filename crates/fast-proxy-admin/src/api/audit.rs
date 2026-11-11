//! The audit log over the API: a paged, filtered list for the UI and an NDJSON
//! export of everything since a time. Entries hold names and ids, never a body,
//! secret, key or PEM (see ADR 0017).

use std::collections::HashMap;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::Json;
use bytes::Bytes;
use fast_proxy_store::repository::audit::{self, AuditEntry, AuditOutcome, AuditQuery};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::input::FieldErrors;
use crate::state::SharedState;
use crate::AppResult;

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;
/// Rows read per query while exporting.
const EXPORT_CHUNK: u32 = 500;

/// One audit entry as the API shows it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntryDto {
    id: i64,
    at_ms: i64,
    instance: String,
    actor: String,
    role: String,
    source: String,
    request_id: String,
    action: String,
    target: String,
    outcome: &'static str,
    /// The structured detail; text if it was not JSON.
    detail: Value,
    revision: Option<u64>,
}

fn dto(row: AuditEntry) -> AuditEntryDto {
    let entry = row.entry;
    let detail = serde_json::from_str(&entry.detail).unwrap_or(Value::String(entry.detail));
    AuditEntryDto {
        id: row.id,
        at_ms: entry.at_ms,
        instance: entry.instance,
        actor: entry.actor,
        role: entry.role,
        source: entry.source,
        request_id: entry.request_id,
        action: entry.action,
        target: entry.target,
        outcome: entry.outcome.as_str(),
        detail,
        revision: entry.revision,
    }
}

/// A page of the log, newest first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditPage {
    data: Vec<AuditEntryDto>,
    /// Pass as `before` for the next (older) page; `None` on the last page.
    next_before: Option<i64>,
    version: u64,
}

/// `GET /api/audit?limit=&before=&actor=&action=&outcome=&since=`
pub async fn list_audit(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<AuditPage>> {
    let mut errors = FieldErrors::default();
    let limit = number(&params, "limit", &mut errors).map_or(DEFAULT_PAGE, |limit| {
        limit.clamp(1, u64::from(MAX_PAGE)) as u32
    });
    let before = number(&params, "before", &mut errors).map(|id| id as i64);
    let since = number(&params, "since", &mut errors).map(|ms| ms as i64);
    let outcome = params
        .get("outcome")
        .and_then(|raw| match AuditOutcome::parse(raw) {
            Some(outcome) => Some(outcome),
            None => {
                errors.add("outcome", "must be ok, denied or failed");
                None
            }
        });
    errors.finish()?;
    let query = AuditQuery {
        actor: params.get("actor").cloned(),
        action: params.get("action").cloned(),
        outcome,
        since_ms: since,
    };
    let rows = state
        .config
        .read(move |conn| audit::list(conn, &query, before, limit))
        .await?
        .value;
    let next_before = (rows.len() == limit as usize)
        .then(|| rows.last().map(|row| row.id))
        .flatten();
    Ok(Json(AuditPage {
        data: rows.into_iter().map(dto).collect(),
        next_before,
        version: state.config.snapshot_holder().load().version,
    }))
}

/// A non-negative integer query parameter, or a field error.
fn number(params: &HashMap<String, String>, name: &str, errors: &mut FieldErrors) -> Option<u64> {
    let raw = params.get(name)?;
    match raw.parse::<u64>() {
        Ok(value) => Some(value),
        Err(_) => {
            errors.add(name, "must be a non-negative whole number");
            None
        }
    }
}

/// `GET /api/audit/export?since=`: every entry since a time (all of them by
/// default), oldest first, one JSON object per line, streamed in chunks.
pub async fn export_audit(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Response> {
    let mut errors = FieldErrors::default();
    let since = number(&params, "since", &mut errors).map(|ms| ms as i64);
    errors.finish()?;
    let (sender, receiver) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let config = state.config.clone();
    tokio::spawn(async move {
        let mut after = 0;
        loop {
            let chunk = config
                .read(move |conn| audit::export(conn, since, after, EXPORT_CHUNK))
                .await;
            let Ok(chunk) = chunk.map(|applied| applied.value) else {
                let _ = sender
                    .send(Err(std::io::Error::other(
                        "the audit log could not be read",
                    )))
                    .await;
                return;
            };
            let Some(last) = chunk.last().map(|row| row.id) else {
                return;
            };
            let mut text = String::new();
            for row in chunk {
                if let Ok(line) = serde_json::to_string(&dto(row)) {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
            if sender.send(Ok(Bytes::from(text))).await.is_err() {
                return;
            }
            after = last;
        }
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"audit.ndjson\""),
    );
    Ok(response)
}
