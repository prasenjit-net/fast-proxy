use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::Json;
use fast_proxy_store::revision;
use serde::Serialize;

use super::config_view::ConfigView;
use super::envelope::Envelope;
use super::input::FieldErrors;
use crate::state::SharedState;
use crate::{AppError, AppResult};

const DEFAULT_PAGE_SIZE: u32 = 50;
const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSummaryDto {
    version: u64,
    created_at_ms: i64,
    summary: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionDetailDto {
    version: u64,
    created_at_ms: i64,
    summary: String,
    config: ConfigView,
}

/// One page of revisions, newest first.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionPage {
    data: Vec<RevisionSummaryDto>,
    /// The current config version.
    version: u64,
    /// Pass as `before` to fetch the next (older) page; absent on the last page.
    next_before: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackResult {
    restored_from: u64,
}

/// Parses `limit` (1-200, default 50) and `before` from the query string,
/// reporting every bad parameter at once.
fn page_params(query: &HashMap<String, String>) -> AppResult<(u32, Option<u64>)> {
    let mut errors = FieldErrors::default();
    let limit = match query.get("limit") {
        None => Some(DEFAULT_PAGE_SIZE),
        Some(raw) => match raw.parse::<u32>() {
            Ok(limit) if (1..=MAX_PAGE_SIZE).contains(&limit) => Some(limit),
            _ => {
                errors.add("limit", "must be a number from 1 to 200");
                None
            }
        },
    };
    let before = match query.get("before") {
        None => Some(None),
        Some(raw) => match raw.parse::<u64>() {
            Ok(version) => Some(Some(version)),
            Err(_) => {
                errors.add("before", "must be a revision number");
                None
            }
        },
    };
    errors.finish()?;
    match (limit, before) {
        (Some(limit), Some(before)) => Ok((limit, before)),
        _ => unreachable!("finish() fails when a parameter is invalid"),
    }
}

/// `GET /api/revisions?limit=&before=`
pub async fn list_revisions(
    State(state): State<SharedState>,
    Query(query): Query<HashMap<String, String>>,
) -> AppResult<Json<RevisionPage>> {
    let (limit, before) = page_params(&query)?;
    let read = state
        .config
        .read(move |conn| revision::list(conn, limit, before))
        .await?;
    let next_before = (read.value.len() == limit as usize)
        .then(|| read.value.last().map(|r| r.version))
        .flatten();
    Ok(Json(RevisionPage {
        data: read
            .value
            .into_iter()
            .map(|r| RevisionSummaryDto {
                version: r.version,
                created_at_ms: r.created_at_ms,
                summary: r.summary,
            })
            .collect(),
        version: read.version,
        next_before,
    }))
}

/// `GET /api/revisions/{version}`: the full configuration as of that revision.
pub async fn get_revision(
    State(state): State<SharedState>,
    Path(version): Path<u64>,
) -> AppResult<Json<Envelope<RevisionDetailDto>>> {
    let read = state
        .config
        .read(move |conn| revision::get(conn, version))
        .await?;
    let found = read.value;
    Ok(Json(Envelope {
        data: RevisionDetailDto {
            version: found.version,
            created_at_ms: found.created_at_ms,
            summary: found.summary,
            config: ConfigView::from_config(&found.config),
        },
        version: read.version,
    }))
}

/// `POST /api/revisions/{version}/rollback`: re-applies that revision as a
/// new one. The change is validated like any other write.
pub async fn rollback_revision(
    State(state): State<SharedState>,
    Path(version): Path<u64>,
) -> AppResult<Json<Envelope<RollbackResult>>> {
    let applied = state
        .config
        .rollback(version)
        .await
        .map_err(AppError::from)?;
    Ok(Json(Envelope {
        data: RollbackResult {
            restored_from: version,
        },
        version: applied.version,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_apply_when_nothing_is_given() {
        assert_eq!(page_params(&query(&[])).unwrap(), (50, None));
    }

    #[test]
    fn accepts_valid_parameters() {
        assert_eq!(
            page_params(&query(&[("limit", "200"), ("before", "7")])).unwrap(),
            (200, Some(7))
        );
    }

    #[test]
    fn reports_every_bad_parameter() {
        for bad in ["0", "201", "x", "-1", ""] {
            let err = page_params(&query(&[("limit", bad), ("before", "no")])).unwrap_err();
            let AppError::Validation(fields) = err else {
                panic!("expected validation errors");
            };
            assert_eq!(fields.len(), 2, "limit={bad:?}");
        }
    }
}
