use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use bytes::Bytes;
use http_body::{Body, Frame, SizeHint};

use fast_proxy_balancing::InFlightGuard;
use fast_proxy_sampling::Sampler;
use fast_proxy_telemetry::{
    AccessLog, AccessRecord, ActiveRequest, DataPlaneFields, Metrics, Plane,
};

use crate::sampling_tail::{Finished, SamplingTail};
use crate::ResponseBody;

/// The shared sinks every request reports to when it finishes.
#[derive(Clone)]
pub struct Observability {
    pub metrics: Arc<Metrics>,
    pub access_log: Arc<AccessLog>,
    /// Where sampled requests go; `None` when sampling is off for the process.
    pub sampler: Option<Arc<Sampler>>,
}

impl Observability {
    /// Metrics and the access log, with no sampling.
    pub fn new(metrics: Arc<Metrics>, access_log: Arc<AccessLog>) -> Self {
        Self {
            metrics,
            access_log,
            sampler: None,
        }
    }

    /// Turns on sampling through `sampler`.
    pub fn with_sampler(mut self, sampler: Arc<Sampler>) -> Self {
        self.sampler = Some(sampler);
        self
    }
}

/// Everything known about a request once its response head is decided.
pub struct RequestSummary {
    pub started: Instant,
    pub client: SocketAddr,
    pub method: String,
    pub uri: String,
    pub version: String,
    pub status: u16,
    pub fields: DataPlaneFields,
    /// What the sampling decision needs, when sampling is on for the request.
    pub(crate) sampling: Option<SamplingTail>,
}

/// Counts the response bytes as they stream to the client, and reports the
/// finished request (metrics and access log) exactly once, when the body is
/// dropped: after its last byte, or when the client went away mid-response.
pub struct TrackedBody {
    inner: ResponseBody,
    bytes_out: u64,
    report: Option<Report>,
}

struct Report {
    summary: RequestSummary,
    observability: Observability,
    /// Keeps the request counted as active until it is reported.
    _active: ActiveRequest,
    /// Keeps the upstream endpoint's in-flight count up until then too.
    _in_flight: Option<InFlightGuard>,
}

impl TrackedBody {
    pub fn new(
        inner: ResponseBody,
        summary: RequestSummary,
        observability: Observability,
        (active, in_flight): (ActiveRequest, Option<InFlightGuard>),
    ) -> Self {
        Self {
            inner,
            bytes_out: 0,
            report: Some(Report {
                summary,
                observability,
                _active: active,
                _in_flight: in_flight,
            }),
        }
    }
}

impl Body for TrackedBody {
    type Data = Bytes;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let polled = Pin::new(&mut self.inner).poll_frame(cx);
        if let Poll::Ready(Some(Ok(frame))) = &polled {
            if let Some(data) = frame.data_ref() {
                self.bytes_out += data.len() as u64;
                let tee = self
                    .report
                    .as_mut()
                    .and_then(|report| report.summary.sampling.as_mut())
                    .and_then(|tail| tail.response_tee.as_mut());
                if let Some(tee) = tee {
                    tee.record(data);
                }
            }
        }
        polled
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for TrackedBody {
    fn drop(&mut self) {
        let Some(report) = self.report.take() else {
            return;
        };
        let Report {
            mut summary,
            observability,
            ..
        } = report;
        let elapsed = summary.started.elapsed();
        let sampling = summary.sampling.take();
        summary.fields.bytes_out = self.bytes_out;
        observability.metrics.observe_request(
            &summary.fields.subsystem,
            &summary.fields.route,
            summary.status,
            elapsed,
        );
        observability.access_log.record(&AccessRecord {
            plane: Plane::Data,
            client: summary.client,
            method: &summary.method,
            uri: &summary.uri,
            version: &summary.version,
            status: summary.status,
            elapsed,
            data_plane: Some(&summary.fields),
            actor: None,
        });
        if let (Some(tail), Some(sampler)) = (sampling, observability.sampler.as_deref()) {
            tail.finish(&Finished {
                metrics: &observability.metrics,
                sampler,
                fields: &summary.fields,
                method: &summary.method,
                uri: &summary.uri,
                client: summary.client,
                status: summary.status,
                elapsed,
                bytes_out: self.bytes_out,
                cancelled: false,
            });
        }
    }
}
