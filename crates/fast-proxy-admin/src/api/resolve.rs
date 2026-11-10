use axum::extract::State;
use axum::http::Uri;
use axum::Json;
use fast_proxy_balancing::monotonic_ms;
use fast_proxy_routing::{
    build_upstream_uri, redirect_target, CompiledRoute, CompiledSubsystem, Lookup, Snapshot,
};
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
    /// `https` when the group has TLS on, else `http`: the scheme the proxy
    /// uses to reach its endpoints.
    upstream_scheme: &'static str,
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
    /// How the matched subsystem is reached over HTTPS; `None` for an unknown host.
    https: Option<HttpsResolution>,
}

/// What HTTPS means for the request's host, as the data plane would act.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpsResolution {
    /// HTTPS is enabled on the subsystem and TLS is on for the process.
    served_over_https: bool,
    /// Set when a request for this host on a plain-HTTP listener would be
    /// redirected (TLS on, HTTPS enabled, a redirect mode set).
    plain_http_redirect: Option<RedirectDto>,
}

#[derive(Debug, Serialize)]
pub struct RedirectDto {
    status: u16,
    location: String,
}

impl Resolution {
    fn unmatched(reason: &'static str, subsystem: Option<NamedRef>) -> Self {
        Self {
            matched: false,
            reason: Some(reason),
            subsystem,
            route: None,
            https: None,
        }
    }
}

fn https_resolution(
    snapshot: &Snapshot,
    subsystem: &CompiledSubsystem,
    host: &str,
    uri: &Uri,
    redirect_port: u16,
) -> HttpsResolution {
    HttpsResolution {
        served_over_https: snapshot.tls_enabled && subsystem.https.enabled,
        plain_http_redirect: redirect_target(snapshot, subsystem, host, uri, redirect_port).map(
            |(status, location)| RedirectDto {
                status: status.as_u16(),
                location,
            },
        ),
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

    let host = body.host.trim();
    let port = state.runtime.tls.redirect_port;
    let https = |subsystem: &CompiledSubsystem| {
        Some(https_resolution(&snapshot, subsystem, host, &uri, port))
    };
    let resolution = match snapshot.lookup(host, path) {
        Lookup::UnknownHost => Resolution::unmatched("unknown_host", None),
        Lookup::SubsystemDisabled { id, subsystem } => Resolution {
            https: https(subsystem),
            ..Resolution::unmatched("subsystem_disabled", Some(named(id.0, subsystem)))
        },
        Lookup::NoRoute { id, subsystem } => Resolution {
            https: https(subsystem),
            ..Resolution::unmatched("no_route", Some(named(id.0, subsystem)))
        },
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
                https: https(subsystem),
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
        upstream_scheme: if upstream.policy.tls.enabled {
            "https"
        } else {
            "http"
        },
    }
}
