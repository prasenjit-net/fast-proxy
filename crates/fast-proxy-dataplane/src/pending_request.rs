//! A request that has not produced a response head yet. If the client leaves
//! first, hyper drops the request's future; without this guard that request
//! would vanish from the metrics and the access log. Dropped before
//! [`PendingRequest::complete`], it reports itself as cancelled (status 499,
//! the convention for "client closed request").

use std::net::SocketAddr;
use std::time::Instant;

use fast_proxy_telemetry::{AccessRecord, ActiveRequest, DataPlaneFields, Plane};

use crate::tracked_body::{Observability, RequestSummary};

/// The status recorded for a request whose client left before the response.
pub const CLIENT_CLOSED: u16 = 499;

struct Inner {
    started: Instant,
    client: SocketAddr,
    method: String,
    uri: String,
    version: String,
    fields: DataPlaneFields,
    active: ActiveRequest,
}

/// Owns everything needed to report a request, until it has a response.
pub struct PendingRequest {
    /// `Some` until `complete` consumes `self`; only `Drop` ever sees `None`.
    inner: Option<Inner>,
    observability: Observability,
}

impl PendingRequest {
    pub fn new(
        (started, client): (Instant, SocketAddr),
        (method, uri, version): (String, String, String),
        fields: DataPlaneFields,
        observability: Observability,
        active: ActiveRequest,
    ) -> Self {
        Self {
            inner: Some(Inner {
                started,
                client,
                method,
                uri,
                version,
                fields,
                active,
            }),
            observability,
        }
    }

    /// What is known about the request so far, to fill in as routing goes.
    pub fn fields_mut(&mut self) -> &mut DataPlaneFields {
        match &mut self.inner {
            Some(inner) => &mut inner.fields,
            None => unreachable!("a pending request is only emptied by complete or drop"),
        }
    }

    /// The response head is decided: hands over what the response body needs
    /// to report the finished request, and disarms the cancellation report.
    pub fn complete(mut self, status: u16) -> (RequestSummary, ActiveRequest) {
        let Some(inner) = self.inner.take() else {
            unreachable!("a pending request is only emptied by complete or drop");
        };
        let summary = RequestSummary {
            started: inner.started,
            client: inner.client,
            method: inner.method,
            uri: inner.uri,
            version: inner.version,
            status,
            fields: inner.fields,
        };
        (summary, inner.active)
    }
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        let elapsed = inner.started.elapsed();
        let fields = &inner.fields;
        let metrics = &self.observability.metrics;
        metrics.observe_request(&fields.subsystem, &fields.route, CLIENT_CLOSED, elapsed);
        metrics.request_cancelled(&fields.subsystem, &fields.route);
        self.observability.access_log.record(&AccessRecord {
            plane: Plane::Data,
            client: inner.client,
            method: &inner.method,
            uri: &inner.uri,
            version: &inner.version,
            status: CLIENT_CLOSED,
            elapsed,
            data_plane: Some(fields),
        });
    }
}
