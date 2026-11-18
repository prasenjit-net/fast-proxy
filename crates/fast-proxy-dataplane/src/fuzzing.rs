//! Entry points for the fuzz targets in `fuzz/` (M7 F1, ADR 0032). Compiled only
//! with the `fuzzing` feature, so none of this is public API: each function is a
//! thin door to a private step of the request path, with no network.

use std::net::IpAddr;

use fast_proxy_model::EndpointAddr;
use fast_proxy_routing::{build_upstream_uri, Snapshot};
use fast_proxy_sampling::UnusualFlags;
use http::{HeaderMap, Method, Request, Uri};

pub use crate::headers::{is_upgrade_request, strip_hop_by_hop, strip_hop_by_hop_keeping_upgrade};
use crate::pipeline::{resolve as resolve_request, Resolved};
use crate::{FixedResponse, RequestLimits};

/// The port plain-HTTP redirects point at, as in production.
const HTTPS_PORT: u16 = 443;

/// What the pipeline decided for a request, without answering it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    AcmeChallenge,
    /// A fixed response, by status.
    Respond(u16),
    Redirect {
        status: u16,
        location: String,
    },
    Maintenance,
    /// Forwarded, with the URI the upstream would see; `None` if the rewrite
    /// could not build one.
    Forward(Option<Uri>),
}

/// The pipeline's resolve step on a plain-HTTP request from `client`; a
/// forwarded request is rewritten for `endpoint`.
pub fn resolve(
    snapshot: &Snapshot,
    request: &Request<()>,
    client: IpAddr,
    endpoint: &EndpointAddr,
) -> Decision {
    let (resolved, _) = resolve_request(snapshot, request, (None, HTTPS_PORT, client));
    match resolved {
        Resolved::AcmeChallenge { .. } => Decision::AcmeChallenge,
        Resolved::Respond { fixed, .. } => Decision::Respond(fixed.status().as_u16()),
        Resolved::Redirect {
            status, location, ..
        } => Decision::Redirect {
            status: status.as_u16(),
            location,
        },
        Resolved::Maintenance { .. } => Decision::Maintenance,
        Resolved::Forward {
            route,
            consumed_prefix_len,
            ..
        } => Decision::Forward(build_upstream_uri(
            route,
            endpoint,
            request.uri(),
            consumed_prefix_len,
        )),
    }
}

/// The size checks before an endpoint is chosen: the body cap that applies, or
/// the status of the refusal (414 or 413).
pub fn check_size(
    uri: &Uri,
    headers: &HeaderMap,
    limits: RequestLimits,
    route_cap: Option<u64>,
) -> Result<Option<u64>, u16> {
    crate::request_size::check(uri, headers, limits, route_cap)
        .map_err(|refusal: FixedResponse| refusal.status().as_u16())
}

/// The sampler's scan of a request head for unusual traits.
pub fn unusual_flags(headers: &HeaderMap, method: &Method, uri: &Uri) -> UnusualFlags {
    crate::sampling_head::unusual_flags(headers, method, uri)
}
