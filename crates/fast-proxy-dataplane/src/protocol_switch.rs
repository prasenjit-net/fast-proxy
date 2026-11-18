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
use crate::progress_io::{self, Deadlines, ProgressIo};
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

    let (guard, force_close, idle) = match context {
        Some(context) => (
            Some(context.guard),
            context.force_close,
            context.tunnel_idle,
        ),
        None => (None, CancellationToken::new(), None),
    };
    tokio::spawn(relay(
        client_upgrade,
        upstream_upgrade,
        (guard, tunnel),
        (force_close, idle),
    ));

    let empty = Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed();
    Response::from_parts(parts, empty)
}

/// Copies bytes between the two upgraded connections. The guard keeps the
/// tunnel counted as an open connection, and as in flight on its endpoint,
/// for as long as it lives. A tunnel that carries nothing either way for
/// `idle` is closed and counted.
async fn relay(
    client: OnUpgrade,
    upstream: OnUpgrade,
    (connection, _in_flight): (Option<std::sync::Arc<ConnectionGuard>>, InFlightGuard),
    (force_close, idle): (CancellationToken, Option<std::time::Duration>),
) {
    let (client, upstream) = tokio::join!(client, upstream);
    let (Ok(client), Ok(upstream)) = (client, upstream) else {
        tracing::debug!("protocol switch was not completed by both sides");
        return;
    };
    // Both sides record into one progress: traffic either way keeps it open.
    let progress = progress_io::new_progress();
    let mut client = ProgressIo::sharing(TokioIo::new(client), progress.clone());
    let mut upstream = ProgressIo::sharing(TokioIo::new(upstream), progress.clone());
    let deadlines = Deadlines {
        first_byte: None,
        send: None,
        idle,
    };
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {
            if let Err(err) = result {
                tracing::debug!("upgraded connection ended with error: {err}");
            }
        }
        stall = progress_io::watch(progress, deadlines) => {
            tracing::debug!(stall = stall.label(), "closing an idle tunnel");
            if let Some(connection) = &connection {
                connection.metrics().client_timeout(stall.label());
            }
        }
        () = force_close.cancelled() => {}
    }
}
