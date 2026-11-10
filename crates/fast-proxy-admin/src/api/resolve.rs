use axum::extract::State;
use axum::http::Uri;
use axum::Json;
use fast_proxy_balancing::monotonic_ms;
use fast_proxy_routing::{build_upstream_uri, CompiledRoute, CompiledSubsystem, Lookup};
use serde::{Deserialize, Serialize};

use super::envelope::Envelope;
use super::input::FieldErrors;
use super::json::ApiJson;
use crate::state::SharedState;
use crate::AppResult;

const MAX_HOST_LEN: usize = 255;

/// Body of `POST /api/resolve`: a request to dry-run against the live config.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveRequest {
    host: String,
    /// Path with an optional query string, for example `/api/users?page=2`.
    path: String,
}

#[derive(Debug, Serialize)]
pub struct NamedRef {
    id: i64,
    name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchedRoute {
    id: i64,
    prefix: String,
    upstream_name: String,
    /// The group's load-balancing algorithm; the endpoint is chosen per request.
    algorithm: &'static str,
    /// Every endpoint with whether it would take a request right now and
    /// the URL the proxy would request from it.
    endpoints: Vec<ResolvedEndpoint>,
    strip_prefix: bool,
    upstream_prefix: String,
    preserve_host: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedEndpoint {
    id: i64,
    addr: String,
    weight: u32,
    state: &'static str,
    /// Admin-enabled, healthy and not ejected at this moment.
    eligible: bool,
    upstream_url: Option<String>,
}

/// What the live configuration would do with the request.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolution {
    matched: bool,
    /// Why nothing matched: `unknown_host`, `subsystem_disabled`, or `no_route`.
    reason: Option<&'static str>,
    subsystem: Option<NamedRef>,
    route: Option<MatchedRoute>,
}

impl Resolution {
    fn unmatched(reason: &'static str, subsystem: Option<NamedRef>) -> Self {
        Self {
            matched: false,
            reason: Some(reason),
            subsystem,
            route: None,
        }
    }
}

fn named(id: i64, subsystem: &CompiledSubsystem) -> NamedRef {
    NamedRef {
        id,
        name: subsystem.name.as_str().to_string(),
    }
}

/// Validates the request and parses the path into a URI.
fn parse_target(body: &ResolveRequest) -> AppResult<Uri> {
    let mut errors = FieldErrors::default();
    if body.host.trim().is_empty() || body.host.len() > MAX_HOST_LEN {
        errors.add("host", "must be 1 to 255 characters");
    }
    let uri = if body.path.starts_with('/') {
        match body.path.parse::<Uri>() {
            Ok(uri) => Some(uri),
            Err(_) => {
                errors.add("path", "is not a valid path");
                None
            }
        }
    } else {
        errors.add("path", "must start with '/'");
        None
    };
    errors.finish()?;
    match uri {
        Some(uri) => Ok(uri),
        None => unreachable!("finish() fails when the path is invalid"),
    }
}

/// `POST /api/resolve`: the same lookup the data plane performs, against
/// the live snapshot, without sending anything.
pub async fn resolve(
    State(state): State<SharedState>,
    ApiJson(body): ApiJson<ResolveRequest>,
) -> AppResult<Json<Envelope<Resolution>>> {
    let uri = parse_target(&body)?;
    let snapshot = state.config.snapshot_holder().load();
    let path = uri.path();

    let resolution = match snapshot.lookup(body.host.trim(), path) {
        Lookup::UnknownHost => Resolution::unmatched("unknown_host", None),
        Lookup::SubsystemDisabled { id, subsystem } => {
            Resolution::unmatched("subsystem_disabled", Some(named(id.0, subsystem)))
        }
        Lookup::NoRoute { id, subsystem } => {
            Resolution::unmatched("no_route", Some(named(id.0, subsystem)))
        }
        Lookup::Matched {
            id,
            subsystem,
            route,
        } => {
            let consumed = path.len() - route.remainder.len();
            Resolution {
                matched: true,
                reason: None,
                subsystem: Some(named(id.0, subsystem)),
                route: Some(matched_route(route.route, &uri, consumed)),
            }
        }
    };
    Ok(Json(Envelope {
        data: resolution,
        version: snapshot.version,
    }))
}

fn matched_route(compiled: &CompiledRoute, uri: &Uri, consumed: usize) -> MatchedRoute {
    let upstream = &compiled.upstream;
    let now = monotonic_ms();
    let endpoints = upstream
        .endpoints
        .iter()
        .zip(&upstream.candidates)
        .map(|(endpoint, candidate)| ResolvedEndpoint {
            id: endpoint.id.0,
            addr: endpoint.addr.to_string(),
            weight: endpoint.weight,
            state: endpoint.state.as_str(),
            eligible: candidate.is_eligible(now),
            upstream_url: build_upstream_uri(compiled, &endpoint.addr, uri, consumed)
                .map(|url| url.to_string()),
        })
        .collect();
    MatchedRoute {
        id: compiled.id.0,
        prefix: compiled.prefix.clone(),
        upstream_name: upstream.name.clone(),
        algorithm: upstream.policy.lb.algorithm.as_str(),
        endpoints,
        strip_prefix: compiled.strip_prefix,
        upstream_prefix: compiled.upstream_prefix.as_str().to_string(),
        preserve_host: compiled.preserve_host,
    }
}
