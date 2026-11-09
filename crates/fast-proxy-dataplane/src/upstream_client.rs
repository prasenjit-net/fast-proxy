use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use fast_proxy_routing::build_upstream_uri;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio_util::sync::CancellationToken;

use crate::connections::{ConnectionContext, ConnectionGuard};
use crate::headers::{self, ClientInfo};
use crate::trusted_proxies::TrustedProxies;
use crate::{FixedResponse, Forward, ResponseBody, Target};
use fast_proxy_telemetry::{Metrics, UpstreamErrorKind};
use std::sync::Arc;

type HttpClient = Client<HttpConnector, Incoming>;

/// How long an unused upstream connection stays pooled.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// Forwards routed requests to their upstream over pooled HTTP/1.1
/// connections, streaming both bodies without buffering.
///
/// The connect timeout belongs to the connector, so one client (and pool)
/// is kept per distinct connect timeout; the response timeout is applied
/// per request.
pub struct UpstreamClient {
    trusted_proxies: TrustedProxies,
    metrics: Arc<Metrics>,
    clients: Mutex<HashMap<Duration, HttpClient>>,
}

impl UpstreamClient {
    pub fn new(trusted_proxies: TrustedProxies, metrics: Arc<Metrics>) -> Self {
        Self {
            trusted_proxies,
            metrics,
            clients: Mutex::new(HashMap::new()),
        }
    }

    fn client_for(&self, connect_timeout: Duration) -> HttpClient {
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        clients
            .entry(connect_timeout)
            .or_insert_with(|| build_client(connect_timeout))
            .clone()
    }
}

impl UpstreamClient {
    /// Counts an upstream failure and builds the matching 502/504 response.
    fn failed(&self, target: &Target<'_>, kind: UpstreamErrorKind) -> Response<ResponseBody> {
        self.metrics
            .upstream_error(target.subsystem.name.as_str(), kind);
        let fixed = match kind {
            UpstreamErrorKind::Timeout => FixedResponse::GatewayTimeout,
            UpstreamErrorKind::Connect | UpstreamErrorKind::Other => FixedResponse::BadGateway,
        };
        fixed.render(target.request_id, target.wants_json)
    }
}

fn build_client(connect_timeout: Duration) -> HttpClient {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(connect_timeout));
    connector.set_nodelay(true);
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_timer(TokioTimer::new())
        // Retries are an M2 feature; a silent retry could replay a request.
        .retry_canceled_requests(false)
        .build(connector)
}

impl Forward for UpstreamClient {
    async fn forward<'a>(
        &'a self,
        mut request: Request<Incoming>,
        target: Target<'a>,
    ) -> Response<ResponseBody> {
        let route = target.route;
        let Some(uri) = build_upstream_uri(
            route,
            &target.endpoint.addr,
            request.uri(),
            target.consumed_prefix_len,
        ) else {
            tracing::warn!(route = route.id.0, "cannot build an upstream URI");
            return self.failed(&target, UpstreamErrorKind::Other);
        };

        // A protocol switch needs the client's side of the socket, which is
        // only available before the request is handed to the upstream client.
        let upgrading = headers::is_upgrade_request(request.headers());
        let context = request.extensions().get::<ConnectionContext>().cloned();
        let client_upgrade = upgrading.then(|| hyper::upgrade::on(&mut request));

        let headers = request.headers_mut();
        if upgrading {
            headers::strip_hop_by_hop_keeping_upgrade(headers);
        } else {
            headers::strip_hop_by_hop(headers);
        }
        headers::apply_forwarded(
            headers,
            &ClientInfo {
                ip: target.client_addr.ip(),
                trusted: self.trusted_proxies.contains(target.client_addr.ip()),
                scheme: "http",
            },
        );
        headers::apply_host(
            headers,
            route.preserve_host,
            &target.endpoint.addr.to_string(),
        );
        headers::apply_request_id(headers, target.request_id);
        *request.uri_mut() = uri;

        let client = self.client_for(route.connect_timeout);
        let outcome = tokio::time::timeout(route.response_timeout, client.request(request)).await;
        match outcome {
            Err(_elapsed) => {
                tracing::warn!(upstream = %target.endpoint.addr, "upstream response timed out");
                self.failed(&target, UpstreamErrorKind::Timeout)
            }
            Ok(Err(err)) => {
                tracing::warn!(upstream = %target.endpoint.addr, "upstream request failed: {err}");
                self.failed(&target, kind_for(err.is_connect(), &err))
            }
            Ok(Ok(response)) => match client_upgrade {
                Some(client_upgrade) if response.status() == StatusCode::SWITCHING_PROTOCOLS => {
                    switch_protocols(response, client_upgrade, context)
                }
                _ => into_client_response(response),
            },
        }
    }
}

/// Streams the upstream response to the client, minus hop-by-hop headers.
fn into_client_response(response: Response<Incoming>) -> Response<ResponseBody> {
    let (mut parts, body) = response.into_parts();
    headers::strip_hop_by_hop(&mut parts.headers);
    let body = body
        .map_err(|err| Box::new(err) as Box<dyn Error + Send + Sync>)
        .boxed();
    Response::from_parts(parts, body)
}

/// The upstream agreed to switch protocols: answer the client with the same
/// `101`, then relay bytes both ways until either side closes (or the drain
/// deadline passes).
fn switch_protocols(
    mut response: Response<Incoming>,
    client_upgrade: OnUpgrade,
    context: Option<ConnectionContext>,
) -> Response<ResponseBody> {
    let upstream_upgrade = hyper::upgrade::on(&mut response);
    let (mut parts, _) = response.into_parts();
    headers::strip_hop_by_hop_keeping_upgrade(&mut parts.headers);

    let (guard, force_close) = match context {
        Some(context) => (Some(context.guard), context.force_close),
        None => (None, CancellationToken::new()),
    };
    tokio::spawn(relay(client_upgrade, upstream_upgrade, guard, force_close));

    let empty = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    Response::from_parts(parts, empty)
}

/// Copies bytes between the two upgraded connections. The guard keeps the
/// tunnel counted as an open connection for as long as it lives.
async fn relay(
    client: OnUpgrade,
    upstream: OnUpgrade,
    _guard: Option<std::sync::Arc<ConnectionGuard>>,
    force_close: CancellationToken,
) {
    let (client, upstream) = tokio::join!(client, upstream);
    let (Ok(client), Ok(upstream)) = (client, upstream) else {
        tracing::debug!("protocol switch was not completed by both sides");
        return;
    };
    let (mut client, mut upstream) = (TokioIo::new(client), TokioIo::new(upstream));
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {
            if let Err(err) = result {
                tracing::debug!("upgraded connection ended with error: {err}");
            }
        }
        () = force_close.cancelled() => {}
    }
}

/// Classifies a failed upstream request: anything caused by a timeout is a
/// timeout (504); otherwise a failure to connect is `Connect` and the rest
/// is `Other` (both 502).
fn kind_for(is_connect: bool, err: &(dyn Error + 'static)) -> UpstreamErrorKind {
    if has_timeout_cause(err) {
        UpstreamErrorKind::Timeout
    } else if is_connect {
        UpstreamErrorKind::Connect
    } else {
        UpstreamErrorKind::Other
    }
}

fn has_timeout_cause(err: &(dyn Error + 'static)) -> bool {
    let mut current = Some(err);
    while let Some(cause) = current {
        if let Some(io_err) = cause.downcast_ref::<io::Error>() {
            if io_err.kind() == io::ErrorKind::TimedOut {
                return true;
            }
        }
        current = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;

    #[derive(Debug)]
    struct Wrapper(Box<dyn Error + Send + Sync>);

    impl fmt::Display for Wrapper {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "wrapper")
        }
    }

    impl Error for Wrapper {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(self.0.as_ref())
        }
    }

    #[test]
    fn a_timed_out_cause_anywhere_in_the_chain_is_a_timeout() {
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "deadline");
        let nested = Wrapper(Box::new(Wrapper(Box::new(timed_out))));
        assert_eq!(kind_for(false, &nested), UpstreamErrorKind::Timeout);
        // A connect that timed out is still a timeout, not a connect error.
        assert_eq!(kind_for(true, &nested), UpstreamErrorKind::Timeout);
    }

    #[test]
    fn a_failed_connect_is_a_connect_error() {
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            kind_for(true, &Wrapper(Box::new(refused))),
            UpstreamErrorKind::Connect
        );
    }

    #[test]
    fn other_failures_are_other() {
        assert_eq!(
            kind_for(false, &io::Error::other("boom")),
            UpstreamErrorKind::Other
        );
    }

    #[test]
    fn one_client_is_kept_per_distinct_connect_timeout() {
        let client = UpstreamClient::new(TrustedProxies::default(), Arc::new(Metrics::new()));
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(2));
        assert_eq!(client.clients.lock().unwrap().len(), 2);
    }
}
