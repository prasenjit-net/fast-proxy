//! The sampled requests over the API (ADR 0022): a filtered, paged list, one
//! sample, an NDJSON export and clearing. They hold request data (redacted, and
//! bodies only where a route opted in), so they need the `Samples` action: an
//! `admin` token. The table is shared by every instance, so any instance lists
//! the samples of all of them. Export and clear are audited.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::HeaderValue;
use axum::response::Response;
use axum::Json;
use bytes::Bytes;
use fast_proxy_config_service::AuditEvent;
use fast_proxy_sample_service::SampleReader;
use fast_proxy_sampling::{Sample, SampleQuery};
use fast_proxy_store::repository::audit::AuditOutcome;
use fast_proxy_store::repository::samples::SampleSummary;
use fast_proxy_store::StoreError;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::change::Change;
use super::envelope::Envelope;
use super::input::{query_number, FieldErrors};
use crate::state::SharedState;
use crate::{AppError, AppResult};

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;
/// Bytes of NDJSON gathered before they are sent on.
const EXPORT_CHUNK_BYTES: usize = 64 * 1024;

/// One sample in a list: the short columns, never the detail.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SampleSummaryDto {
    id: i64,
    at_ms: i64,
    instance: String,
    request_id: String,
    triggers: Vec<&'static str>,
    primary: &'static str,
    subsystem: String,
    route: String,
    client: String,
    method: String,
    status: u16,
    total_ms: u32,
    size_bytes: u64,
}

fn summary_dto(row: SampleSummary) -> SampleSummaryDto {
    SampleSummaryDto {
        id: row.id,
        at_ms: row.at_ms,
        instance: row.instance,
        request_id: row.request_id,
        triggers: row
            .triggers
            .iter()
            .map(fast_proxy_model::Trigger::as_str)
            .collect(),
        primary: row.primary.as_str(),
        subsystem: row.subsystem,
        route: row.route,
        client: row.client,
        method: row.method,
        status: row.status,
        total_ms: row.total_ms,
        size_bytes: row.size_bytes,
    }
}

/// A page of the list, newest first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplePage {
    data: Vec<SampleSummaryDto>,
    /// Pass as `before` for the next (older) page; `None` on the last page.
    next_before: Option<i64>,
    version: u64,
}

/// Runs a read on the blocking pool: the reader's calls wait on SQLite.
async fn blocking<T: Send + 'static>(
    reader: &Arc<SampleReader>,
    read: impl FnOnce(&SampleReader) -> Result<T, StoreError> + Send + 'static,
) -> AppResult<T> {
    let reader = reader.clone();
    tokio::task::spawn_blocking(move || read(&reader))
        .await
        .map_err(|err| AppError::Internal(err.to_string()))?
        .map_err(AppError::from)
}

/// The filters of a request's query string: `trigger` (a comma separated list),
/// `statusClass`, `subsystem`, `route`, `instance`, `client`, `minMs`, `since`,
/// `before` and `limit`.
fn query_of(params: &HashMap<String, String>) -> AppResult<SampleQuery> {
    let mut errors = FieldErrors::default();
    let text = |name: &str| params.get(name).filter(|v| !v.is_empty()).cloned();
    let number = |name: &str, errors: &mut FieldErrors| query_number(params, name, errors);
    let triggers = params
        .get("trigger")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let query = SampleQuery {
        triggers,
        status_class: number("statusClass", &mut errors).and_then(|n| u8::try_from(n).ok()),
        subsystem: text("subsystem"),
        route: text("route"),
        instance: text("instance"),
        client: text("client"),
        min_ms: number("minMs", &mut errors).map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
        since_ms: number("since", &mut errors).map(|n| n as i64),
        before_id: number("before", &mut errors).map(|n| n as i64),
        limit: Some(
            number("limit", &mut errors)
                .map_or(DEFAULT_PAGE, |n| n.clamp(1, u64::from(MAX_PAGE)) as u32),
        ),
    };
    if params.get("statusClass").is_some() && query.status_class.is_none() {
        errors.add("statusClass", "must be between 1 and 5");
    }
    if let Err(err) = query.validate() {
        errors.add(&err.field, &err.message);
    }
    errors.finish()?;
    Ok(query)
}

/// `GET /api/samples`
pub async fn list_samples(
    State(state): State<SharedState>,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Json<SamplePage>> {
    let query = query_of(&params)?;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE) as usize;
    let rows = blocking(&state.samples, move |reader| reader.list(&query)).await?;
    let next_before = (rows.len() == limit)
        .then(|| rows.last().map(|row| row.id))
        .flatten();
    Ok(Json(SamplePage {
        data: rows.into_iter().map(summary_dto).collect(),
        next_before,
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `GET /api/samples/{id}`: one sample with its headers, bodies and attempts.
pub async fn get_sample(
    State(state): State<SharedState>,
    Path(id): Path<i64>,
) -> AppResult<Json<Envelope<Sample>>> {
    let sample = blocking(&state.samples, move |reader| reader.get(id)).await?;
    Ok(Json(Envelope {
        data: sample,
        version: state.config.snapshot_holder().load().version,
    }))
}

/// `GET /api/samples/export`: every sample matching the filters (the same as
/// the list's, without paging), newest first, one JSON object per line, read in
/// pages so a large table is never held in memory. The export is audited.
pub async fn export_samples(
    State(state): State<SharedState>,
    Change(context): Change,
    Query(params): Query<HashMap<String, String>>,
) -> AppResult<Response> {
    let query = query_of(&params)?;
    let filters: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .filter(|(name, _)| !matches!(name.as_str(), "limit" | "before"))
        .map(|(name, value)| (name.clone(), serde_json::Value::String(value.clone())))
        .collect();
    state
        .config
        .record_audit(
            &context,
            AuditEvent {
                action: "samples.export",
                target: "samples".into(),
                outcome: AuditOutcome::Ok,
                detail: serde_json::json!({ "filters": filters }),
            },
        )
        .await;
    let (sender, receiver) = mpsc::channel::<Result<Bytes, std::io::Error>>(4);
    let reader = state.samples.clone();
    tokio::task::spawn_blocking(move || stream_ndjson(&reader, &query, &sender));
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(receiver)));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"samples.ndjson\""),
    );
    Ok(response)
}

/// Sends the matching samples as NDJSON chunks; stops when the client is gone.
fn stream_ndjson(
    reader: &SampleReader,
    query: &SampleQuery,
    sender: &mpsc::Sender<Result<Bytes, std::io::Error>>,
) {
    let mut chunk = String::new();
    let flush = |chunk: &mut String| {
        let bytes = Bytes::from(std::mem::take(chunk));
        sender.blocking_send(Ok(bytes)).map_err(|_| ())
    };
    let exported = reader.export_each(query, |sample| {
        chunk.push_str(&sample.to_detail_json());
        chunk.push('\n');
        if chunk.len() >= EXPORT_CHUNK_BYTES {
            return flush(&mut chunk);
        }
        Ok(())
    });
    match exported {
        Ok(Ok(())) if !chunk.is_empty() => {
            let _ = flush(&mut chunk);
        }
        Ok(_) => {}
        Err(_) => {
            let _ =
                sender.blocking_send(Err(std::io::Error::other("the samples could not be read")));
        }
    }
}

/// What clearing the samples did.
#[derive(Debug, Serialize)]
pub struct Cleared {
    deleted: u64,
}

/// `DELETE /api/samples`: deletes every stored sample, whichever instance took
/// it. Audited, with how many.
pub async fn clear_samples(
    State(state): State<SharedState>,
    Change(context): Change,
) -> AppResult<Json<Cleared>> {
    let deleted = blocking(&state.samples, SampleReader::clear).await?;
    state
        .config
        .record_audit(
            &context,
            AuditEvent {
                action: "samples.clear",
                target: "samples".into(),
                outcome: AuditOutcome::Ok,
                detail: serde_json::json!({ "deleted": deleted }),
            },
        )
        .await;
    state.sampling_changed("cleared");
    Ok(Json(Cleared { deleted }))
}
