use std::net::SocketAddr;
use std::sync::Arc;

use fast_proxy_routing::{CompiledRoute, CompiledSubsystem, Lookup, Snapshot, SnapshotHolder};
use http::{header, Request, Response};
use hyper::body::Incoming;

use crate::request_id::{header_value, request_id_for, REQUEST_ID};
use crate::responses::accepts_json;
use crate::{FixedResponse, Forward, ResponseBody, Target};

/// What the snapshot says should happen to a request.
enum Resolved<'snapshot> {
    Respond(FixedResponse),
    Forward {
        subsystem: &'snapshot CompiledSubsystem,
        route: &'snapshot CompiledRoute,
        consumed_prefix_len: usize,
    },
}

/// Turns one request into one response: host to subsystem to route to
/// upstream. The snapshot is loaded once, so a config swap mid-request
/// never changes how that request is routed.
pub struct RequestPipeline<F> {
    snapshots: Arc<SnapshotHolder>,
    forwarder: F,
}

impl<F: Forward> RequestPipeline<F> {
    pub fn new(snapshots: Arc<SnapshotHolder>, forwarder: F) -> Self {
        Self {
            snapshots,
            forwarder,
        }
    }

    /// Handles one request. Never fails: every problem becomes a response.
    pub async fn handle(
        &self,
        request: Request<Incoming>,
        client_addr: SocketAddr,
    ) -> Response<ResponseBody> {
        let snapshot = self.snapshots.load();
        let request_id = request_id_for(request.headers());
        let wants_json = accepts_json(request.headers());

        let (subsystem, route, consumed_prefix_len) = match resolve(&snapshot, &request) {
            Resolved::Respond(fixed) => return fixed.render(&request_id, wants_json),
            Resolved::Forward {
                subsystem,
                route,
                consumed_prefix_len,
            } => (subsystem, route, consumed_prefix_len),
        };

        let target = Target {
            subsystem,
            route,
            consumed_prefix_len,
            request_id: &request_id,
            client_addr,
            wants_json,
        };
        let mut response = self.forwarder.forward(request, target).await;
        response
            .headers_mut()
            .insert(REQUEST_ID, header_value(&request_id));
        response
    }
}

fn resolve<'snapshot>(
    snapshot: &'snapshot Snapshot,
    request: &Request<Incoming>,
) -> Resolved<'snapshot> {
    let path = request.uri().path();
    match snapshot.lookup(host_of(request), path) {
        Lookup::UnknownHost => Resolved::Respond(FixedResponse::UnknownHost),
        Lookup::SubsystemDisabled { .. } => Resolved::Respond(FixedResponse::SubsystemDisabled),
        Lookup::NoRoute { .. } => Resolved::Respond(FixedResponse::NoRoute),
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
