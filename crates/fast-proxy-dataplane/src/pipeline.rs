use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use fast_proxy_routing::{
    redirect_target, CompiledMaintenance, CompiledRoute, CompiledSubsystem, Lookup, Snapshot,
    SnapshotHolder,
};
use fast_proxy_telemetry::{DataPlaneFields, Metrics, NO_ROUTE, UNMATCHED_SUBSYSTEM};
use http::{header, Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;

use crate::connections::{ConnectionContext, TlsConnection};
use crate::maintenance::{self, evaluate, BypassedScopes, ScopeOutcome, BYPASS_HEADER};
use crate::pending_request::PendingRequest;
use crate::redirect::{hsts_value, render_redirect};
use crate::request_id::{header_value, request_id_for, REQUEST_ID};
use crate::responses::accepts_json;
use crate::selection::select_endpoint;
use crate::tracked_body::TrackedBody;
use crate::trusted_proxies::TrustedProxies;
use crate::{FixedResponse, Forward, Observability, ResponseBody, Target};

/// What the snapshot says should happen to a request.
enum Resolved<'snapshot> {
    Respond {
        fixed: FixedResponse,
        /// The subsystem the host resolved to, if any (for labels and logs).
        subsystem: Option<&'snapshot CompiledSubsystem>,
    },
    /// A plain-HTTP request for a subsystem that sends clients to HTTPS.
    Redirect {
        status: http::StatusCode,
        location: String,
        subsystem: &'snapshot CompiledSubsystem,
    },
    /// A maintenance scope answers: nothing is routed, no endpoint is touched.
    Maintenance {
        scope: &'snapshot CompiledMaintenance,
        subsystem: &'snapshot CompiledSubsystem,
        /// The route, when the scope that answered is the route's own.
        route: Option<&'snapshot CompiledRoute>,
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
    /// The port plain-HTTP redirects point at; 443 unless told otherwise.
    https_port: u16,
    /// Peers whose `X-Forwarded-For` is believed when deciding who the client
    /// is for a maintenance bypass.
    trusted_proxies: TrustedProxies,
}

impl<F: Forward> RequestPipeline<F> {
    pub fn new(snapshots: Arc<SnapshotHolder>, forwarder: F, observability: Observability) -> Self {
        Self {
            snapshots,
            forwarder,
            observability,
            https_port: 443,
            trusted_proxies: TrustedProxies::default(),
        }
    }

    /// Sets the port a redirect to HTTPS names (the public port of the HTTPS
    /// listener); it is left out of the URL when it is 443.
    pub fn with_https_port(mut self, port: u16) -> Self {
        self.https_port = port;
        self
    }

    /// Sets the trusted proxies, so a maintenance bypass by network sees the
    /// client behind them and never a forged `X-Forwarded-For` from anyone else.
    pub fn with_trusted_proxies(mut self, trusted_proxies: TrustedProxies) -> Self {
        self.trusted_proxies = trusted_proxies;
        self
    }

    /// The metrics every request reports to.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.observability.metrics.clone()
    }

    /// Handles one request. Never fails: every problem becomes a response.
    pub async fn handle(
        &self,
        mut request: Request<Incoming>,
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

        let tls = request
            .extensions()
            .get::<ConnectionContext>()
            .and_then(|context| context.tls.clone());
        let fields = DataPlaneFields {
            proto: protocol_of(request.version()).to_string(),
            tls: tls.as_ref().map(|tls| tls.version.clone()),
            subsystem: UNMATCHED_SUBSYSTEM.to_string(),
            route: NO_ROUTE.to_string(),
            upstream: None,
            attempts: 0,
            request_id: request_id.clone(),
            upstream_time: None,
            bytes_out: 0,
            maintenance: None,
        };
        // If the client leaves before there is a response, dropping this
        // reports the request as cancelled (499) instead of losing it.
        let mut pending = PendingRequest::new(
            (started, client_addr),
            (method, uri, version),
            fields,
            self.observability.clone(),
            active,
        );

        // Keeps the chosen endpoint's in-flight count up until the response
        // body has been fully sent (or the client went away).
        let mut in_flight = None;
        let mut hsts_from = None;
        let client = self
            .trusted_proxies
            .effective_client(client_addr.ip(), request.headers());
        let (resolved, bypassed) = resolve(
            &snapshot,
            &request,
            (tls.as_deref(), self.https_port, client),
        );
        let mut response = match resolved {
            Resolved::Respond { fixed, subsystem } => {
                if let Some(subsystem) = subsystem {
                    pending.fields_mut().subsystem = subsystem.name.as_str().to_string();
                    hsts_from = Some(subsystem);
                }
                fixed.render(&request_id, wants_json)
            }
            Resolved::Redirect {
                status,
                location,
                subsystem,
            } => {
                pending.fields_mut().subsystem = subsystem.name.as_str().to_string();
                render_redirect(status, &location, &request_id)
                    .unwrap_or_else(|| FixedResponse::UnknownHost.render(&request_id, wants_json))
            }
            Resolved::Maintenance {
                scope,
                subsystem,
                route,
            } => {
                hsts_from = Some(subsystem);
                let known = pending.fields_mut();
                known.subsystem = subsystem.name.as_str().to_string();
                if let Some(route) = route {
                    known.route = route.prefix.clone();
                }
                known.maintenance = Some(scope.scope.label().to_string());
                self.observability.metrics.maintenance_response(
                    scope.scope.label(),
                    &known.subsystem,
                    &known.route,
                );
                maintenance::render(scope, request.headers(), &request_id)
            }
            Resolved::Forward {
                subsystem,
                route,
                consumed_prefix_len,
            } => {
                hsts_from = Some(subsystem);
                let known = pending.fields_mut();
                known.subsystem = subsystem.name.as_str().to_string();
                known.route = route.prefix.clone();
                let upstream = &route.upstream;
                bypassed.strip_secrets(request.headers_mut());
                match select_endpoint(upstream, request.headers(), client_addr.ip(), None) {
                    None => FixedResponse::NoHealthyUpstream.render(&request_id, wants_json),
                    Some(selection) => {
                        let endpoint = &upstream.endpoints[selection.index];
                        let candidate = &upstream.candidates[selection.index];
                        if selection.panic {
                            self.observability
                                .metrics
                                .upstream_panic(subsystem.name.as_str(), &upstream.name);
                        }
                        // Known from the start, so a request cancelled while it
                        // waits still shows which endpoint it was sent to.
                        let known = pending.fields_mut();
                        known.upstream = Some(endpoint.addr.to_string());
                        known.attempts = 1;
                        let target = Target {
                            subsystem,
                            route,
                            endpoint,
                            endpoint_index: selection.index,
                            candidate,
                            consumed_prefix_len,
                            request_id: &request_id,
                            client_addr,
                            wants_json,
                        };
                        let forwarding = Instant::now();
                        let forwarded = self.forwarder.forward(request, target).await;
                        let known = pending.fields_mut();
                        known.upstream_time = Some(forwarding.elapsed());
                        known.upstream = Some(
                            upstream.endpoints[forwarded.endpoint_index]
                                .addr
                                .to_string(),
                        );
                        known.attempts = forwarded.attempts;
                        in_flight = forwarded.in_flight;
                        let mut response = forwarded.response;
                        response
                            .headers_mut()
                            .insert(REQUEST_ID, header_value(&request_id));
                        response
                    }
                }
            }
        };

        // A request that passed through maintenance says so, and is counted.
        if let Some(value) = bypassed.header_value() {
            response.headers_mut().insert(BYPASS_HEADER, value);
            let subsystem = pending.fields_mut().subsystem.clone();
            for scope in bypassed.scopes() {
                self.observability
                    .metrics
                    .maintenance_bypass(scope.label(), &subsystem);
            }
        }

        // HSTS is only ever sent over HTTPS (RFC 6797).
        if tls.is_some() {
            if let Some(value) = hsts_from.and_then(|subsystem| hsts_value(&snapshot, subsystem)) {
                response
                    .headers_mut()
                    .insert(header::STRICT_TRANSPORT_SECURITY, value);
            }
        }

        let (summary, active) = pending.complete(response.status().as_u16());
        let (parts, body) = response.into_parts();
        let tracked = TrackedBody::new(
            body,
            summary,
            self.observability.clone(),
            (active, in_flight),
        );
        Response::from_parts(parts, tracked.boxed())
    }
}

/// The access log's protocol name for a request.
fn protocol_of(version: http::Version) -> &'static str {
    match version {
        http::Version::HTTP_2 => "h2",
        http::Version::HTTP_10 => "http/1.0",
        _ => "http/1.1",
    }
}

/// Whether a request on an HTTPS connection may not be served by it (421): its
/// subsystem is not served over HTTPS at all, or its certificate differs from
/// the one the handshake presented for the client's SNI. Hosts sharing one
/// certificate pass, which keeps HTTP/2 connection coalescing working; a client
/// that sent no SNI skips the second rule.
fn is_misdirected(subsystem: &CompiledSubsystem, tls: &TlsConnection) -> bool {
    if !subsystem.https.enabled {
        return true;
    }
    tls.sni.is_some() && tls.certificate_id != subsystem.https.certificate_id
}

/// Routes a request. `connection` is the TLS state of the connection, the
/// port redirects point at, and the effective client address.
fn resolve<'snapshot>(
    snapshot: &'snapshot Snapshot,
    request: &Request<Incoming>,
    (tls, https_port, client): (Option<&TlsConnection>, u16, std::net::IpAddr),
) -> (Resolved<'snapshot>, BypassedScopes) {
    let path = request.uri().path();
    let host = host_of(request);
    let lookup = snapshot.lookup(host, path);
    // On a plain-HTTP listener, a matched subsystem may send clients to HTTPS
    // before anything is routed.
    if tls.is_none() {
        if let Lookup::SubsystemDisabled { subsystem, .. }
        | Lookup::NoRoute { subsystem, .. }
        | Lookup::Matched { subsystem, .. } = &lookup
        {
            if let Some((status, location)) =
                redirect_target(snapshot, subsystem, host, request.uri(), https_port)
            {
                return (
                    Resolved::Redirect {
                        status,
                        location,
                        subsystem,
                    },
                    BypassedScopes::default(),
                );
            }
        }
    }
    if let (
        Some(tls),
        Lookup::SubsystemDisabled { subsystem, .. }
        | Lookup::NoRoute { subsystem, .. }
        | Lookup::Matched { subsystem, .. },
    ) = (tls, &lookup)
    {
        if is_misdirected(subsystem, tls) {
            return (
                Resolved::Respond {
                    fixed: FixedResponse::MisdirectedRequest,
                    subsystem: Some(subsystem),
                },
                BypassedScopes::default(),
            );
        }
    }
    // Maintenance comes after the 421 and redirect rules and before routing:
    // the whole proxy's scope, then the subsystem's.
    let mut bypassed = BypassedScopes::default();
    if let Lookup::SubsystemDisabled { subsystem, .. }
    | Lookup::NoRoute { subsystem, .. }
    | Lookup::Matched { subsystem, .. } = &lookup
    {
        let scopes = [
            snapshot.maintenance.as_deref(),
            subsystem.maintenance.as_deref(),
        ];
        match evaluate(&scopes, request.headers(), client, bypassed) {
            ScopeOutcome::Respond(scope) => {
                return (
                    Resolved::Maintenance {
                        scope,
                        subsystem,
                        route: None,
                    },
                    BypassedScopes::default(),
                );
            }
            ScopeOutcome::Pass(passed) => bypassed = passed,
        }
    }
    let resolved = match lookup {
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
        } => {
            // The route's own scope, once the route is known.
            match evaluate(
                &[route.route.maintenance.as_deref()],
                request.headers(),
                client,
                bypassed,
            ) {
                ScopeOutcome::Respond(scope) => {
                    return (
                        Resolved::Maintenance {
                            scope,
                            subsystem,
                            route: Some(route.route),
                        },
                        BypassedScopes::default(),
                    );
                }
                ScopeOutcome::Pass(passed) => bypassed = passed,
            }
            Resolved::Forward {
                subsystem,
                route: route.route,
                consumed_prefix_len: path.len() - route.remainder.len(),
            }
        }
    };
    (resolved, bypassed)
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
