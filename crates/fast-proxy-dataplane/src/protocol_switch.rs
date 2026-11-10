//! Relaying an upstream response to the client, including the switch to a
//! tunnelled protocol (WebSocket).

use std::error::Error;

use bytes::Bytes;
use fast_proxy_balancing::InFlightGuard;
use http::Response;
use http_body_util::{BodyExt, Empty};
use hyper::body::Incoming;
use hyper::upgrade::OnUpgrade;
use hyper_util::rt::TokioIo;
use tokio_util::sync::CancellationToken;

use crate::connections::{ConnectionContext, ConnectionGuard};
use crate::headers;
use crate::ResponseBody;

/// Streams the upstream response to the client, minus hop-by-hop headers.
pub(crate) fn into_client_response(response: Response<Incoming>) -> Response<ResponseBody> {
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
pub(crate) fn switch_protocols(
    mut response: Response<Incoming>,
    client_upgrade: OnUpgrade,
    context: Option<ConnectionContext>,
    tunnel: InFlightGuard,
) -> Response<ResponseBody> {
    let upstream_upgrade = hyper::upgrade::on(&mut response);
    let (mut parts, _) = response.into_parts();
    headers::strip_hop_by_hop_keeping_upgrade(&mut parts.headers);

    let (guard, force_close) = match context {
        Some(context) => (Some(context.guard), context.force_close),
        None => (None, CancellationToken::new()),
    };
    tokio::spawn(relay(
        client_upgrade,
        upstream_upgrade,
        (guard, tunnel),
        force_close,
    ));

    let empty = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    Response::from_parts(parts, empty)
}

/// Copies bytes between the two upgraded connections. The guard keeps the
/// tunnel counted as an open connection, and as in flight on its endpoint,
/// for as long as it lives.
async fn relay(
    client: OnUpgrade,
    upstream: OnUpgrade,
    _guards: (Option<std::sync::Arc<ConnectionGuard>>, InFlightGuard),
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
