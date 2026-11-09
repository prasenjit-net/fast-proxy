use std::collections::HashMap;
use std::error::Error;
use std::io;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use fast_proxy_routing::build_upstream_uri;
use http::{Request, Response};
use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioTimer};

use crate::headers::{self, ClientInfo};
use crate::trusted_proxies::TrustedProxies;
use crate::{FixedResponse, Forward, ResponseBody, Target};

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
    clients: Mutex<HashMap<Duration, HttpClient>>,
}

impl UpstreamClient {
    pub fn new(trusted_proxies: TrustedProxies) -> Self {
        Self {
            trusted_proxies,
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
        let Some(uri) = build_upstream_uri(route, request.uri(), target.consumed_prefix_len) else {
            tracing::warn!(route = route.id.0, "cannot build an upstream URI");
            return FixedResponse::BadGateway.render(target.request_id, target.wants_json);
        };

        let headers = request.headers_mut();
        headers::strip_hop_by_hop(headers);
        headers::apply_forwarded(
            headers,
            &ClientInfo {
                ip: target.client_addr.ip(),
                trusted: self.trusted_proxies.contains(target.client_addr.ip()),
                scheme: "http",
            },
        );
        headers::apply_host(headers, route.preserve_host, &route.endpoint.to_string());
        headers::apply_request_id(headers, target.request_id);
        *request.uri_mut() = uri;

        let client = self.client_for(route.connect_timeout);
        let outcome = tokio::time::timeout(route.response_timeout, client.request(request)).await;
        match outcome {
            Err(_elapsed) => {
                tracing::warn!(upstream = %route.endpoint, "upstream response timed out");
                FixedResponse::GatewayTimeout.render(target.request_id, target.wants_json)
            }
            Ok(Err(err)) => {
                let failure = failure_for(&err);
                tracing::warn!(upstream = %route.endpoint, "upstream request failed: {err}");
                failure.render(target.request_id, target.wants_json)
            }
            Ok(Ok(response)) => into_client_response(response),
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

/// A connect that timed out is a 504; any other failure is a 502.
fn failure_for(err: &(dyn Error + 'static)) -> FixedResponse {
    if has_timeout_cause(err) {
        FixedResponse::GatewayTimeout
    } else {
        FixedResponse::BadGateway
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
    fn a_timed_out_cause_anywhere_in_the_chain_is_a_gateway_timeout() {
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "deadline");
        let nested = Wrapper(Box::new(Wrapper(Box::new(timed_out))));
        assert_eq!(failure_for(&nested), FixedResponse::GatewayTimeout);
    }

    #[test]
    fn other_failures_are_a_bad_gateway() {
        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert_eq!(
            failure_for(&Wrapper(Box::new(refused))),
            FixedResponse::BadGateway
        );
        assert_eq!(
            failure_for(&io::Error::other("boom")),
            FixedResponse::BadGateway
        );
    }

    #[test]
    fn one_client_is_kept_per_distinct_connect_timeout() {
        let client = UpstreamClient::new(TrustedProxies::default());
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(1));
        client.client_for(Duration::from_secs(2));
        assert_eq!(client.clients.lock().unwrap().len(), 2);
    }
}
