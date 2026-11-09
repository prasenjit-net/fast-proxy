use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use fast_proxy_routing::{CompiledRoute, CompiledSubsystem, Lookup, Snapshot, SnapshotHolder};
use fast_proxy_telemetry::{DataPlaneFields, Metrics, NO_ROUTE, UNMATCHED_SUBSYSTEM};
use http::{header, Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;

use crate::request_id::{header_value, request_id_for, REQUEST_ID};
use crate::responses::accepts_json;
use crate::tracked_body::{RequestSummary, TrackedBody};
use crate::{FixedResponse, Forward, Observability, ResponseBody, Target};

/// What the snapshot says should happen to a request.
enum Resolved<'snapshot> {
    Respond {
        fixed: FixedResponse,
        /// The subsystem the host resolved to, if any (for labels and logs).
        subsystem: Option<&'snapshot CompiledSubsystem>,
    },
    Forward {
        subsystem: &'snapshot CompiledSubsystem,
        route: &'snapshot CompiledRoute,
        consumed_prefix_len: usize,
    },
}

/// Turns one request into one response: host to subsystem to route to
/// upstream. The snapshot is loaded once, so a config swap mid-request
/// never changes how that request is routed. Every request is counted in
/// the metrics and written to the access log when its response finishes.
pub struct RequestPipeline<F> {
    snapshots: Arc<SnapshotHolder>,
    forwarder: F,
    observability: Observability,
}

impl<F: Forward> RequestPipeline<F> {
    pub fn new(snapshots: Arc<SnapshotHolder>, forwarder: F, observability: Observability) -> Self {
        Self {
            snapshots,
            forwarder,
            observability,
        }
    }

    /// The metrics every request reports to.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.observability.metrics.clone()
    }

    /// Handles one request. Never fails: every problem becomes a response.
    pub async fn handle(
        &self,
        request: Request<Incoming>,
        client_addr: SocketAddr,
    ) -> Response<ResponseBody> {
        let started = Instant::now();
        let active = self.observability.metrics.request_started();
        let snapshot = self.snapshots.load();
        let request_id = request_id_for(request.headers());
        let wants_json = accepts_json(request.headers());
        let (method, uri, version) = (
            request.method().to_string(),
            request.uri().to_string(),
            format!("{:?}", request.version()),
        );

        let mut fields = DataPlaneFields {
            subsystem: UNMATCHED_SUBSYSTEM.to_string(),
            route: NO_ROUTE.to_string(),
            upstream: None,
            request_id: request_id.clone(),
            upstream_time: None,
            bytes_out: 0,
        };

        let response = match resolve(&snapshot, &request) {
            Resolved::Respond { fixed, subsystem } => {
                if let Some(subsystem) = subsystem {
                    fields.subsystem = subsystem.name.as_str().to_string();
                }
                fixed.render(&request_id, wants_json)
            }
            Resolved::Forward {
                subsystem,
                route,
                consumed_prefix_len,
            } => {
                fields.subsystem = subsystem.name.as_str().to_string();
                fields.route = route.prefix.clone();
                fields.upstream = Some(route.endpoint.to_string());
                let target = Target {
                    subsystem,
                    route,
                    consumed_prefix_len,
                    request_id: &request_id,
                    client_addr,
                    wants_json,
                };
                let forwarding = Instant::now();
                let mut response = self.forwarder.forward(request, target).await;
                fields.upstream_time = Some(forwarding.elapsed());
                response
                    .headers_mut()
                    .insert(REQUEST_ID, header_value(&request_id));
                response
            }
        };

        let summary = RequestSummary {
            started,
            client: client_addr,
            method,
            uri,
            version,
            status: response.status().as_u16(),
            fields,
        };
        let (parts, body) = response.into_parts();
        let tracked = TrackedBody::new(body, summary, self.observability.clone(), active);
        Response::from_parts(parts, tracked.boxed())
    }
}

fn resolve<'snapshot>(
    snapshot: &'snapshot Snapshot,
    request: &Request<Incoming>,
) -> Resolved<'snapshot> {
    let path = request.uri().path();
    match snapshot.lookup(host_of(request), path) {
        Lookup::UnknownHost => Resolved::Respond {
            fixed: FixedResponse::UnknownHost,
            subsystem: None,
        },
        Lookup::SubsystemDisabled { subsystem, .. } => Resolved::Respond {
            fixed: FixedResponse::SubsystemDisabled,
            subsystem: Some(subsystem),
        },
        Lookup::NoRoute { subsystem, .. } => Resolved::Respond {
            fixed: FixedResponse::NoRoute,
            subsystem: Some(subsystem),
        },
        Lookup::Matched {
            subsystem, route, ..
        } => Resolved::Forward {
            subsystem,
            route: route.route,
            consumed_prefix_len: path.len() - route.remainder.len(),
        },
    }
}

/// The request's host: the `Host` header, else the URI authority (absolute
/// form), else empty, which matches no subsystem.
fn host_of(request: &Request<Incoming>) -> &str {
    request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        })
        .unwrap_or("")
}
