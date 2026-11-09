//! Protocol-switch (WebSocket) passthrough through the real proxy, with a
//! real WebSocket server upstream and a real WebSocket client.

use std::time::Duration;

use bytes::Bytes;
use fast_proxy_model::Config;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{client_async, MaybeTlsStream, WebSocketStream};

use crate::test_support::{eventually, route, send_raw, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream, WsEchoServer};
use crate::{TrustedProxies, UpstreamClient};

const HOST: &str = "shop.example.com";

pub type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// A proxy in front of the given upstream address.
pub async fn proxy_for(upstream: std::net::SocketAddr) -> Harness {
    let config = Config {
        subsystems: vec![site_to(1, "shop", HOST, upstream, vec![route(1, "/")])],
    };
    Harness::start(config, UpstreamClient::new(TrustedProxies::default())).await
}

/// Opens a WebSocket to `ws://shop.example.com/chat` through the proxy.
pub async fn connect(proxy: &Harness) -> ClientSocket {
    let stream = TcpStream::connect(proxy.addr).await.unwrap();
    let request = format!("ws://{HOST}/chat").into_client_request().unwrap();
    let (socket, response) = client_async(request, MaybeTlsStream::Plain(stream))
        .await
        .expect("the WebSocket handshake should succeed through the proxy");
    assert_eq!(response.status(), 101);
    socket
}

#[tokio::test]
async fn a_websocket_echoes_through_the_proxy() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;

    socket.send(Message::text("hello")).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::text("hello")
    );

    socket
        .send(Message::Binary(Bytes::from_static(&[1, 2, 3])))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(Bytes::from_static(&[1, 2, 3]))
    );

    for n in 0..50 {
        socket
            .send(Message::text(format!("msg {n}")))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::text(format!("msg {n}"))
        );
    }
    socket.close(None).await.unwrap();
}

#[tokio::test]
async fn the_upstream_handshake_carries_the_websocket_and_forwarded_headers() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;
    socket.send(Message::text("ping")).await.unwrap();
    socket.next().await.unwrap().unwrap();

    let handshakes = server.handshakes.lock().unwrap();
    let seen = &handshakes[0];
    assert_eq!(seen["upgrade"], "websocket");
    assert!(seen["connection"]
        .to_str()
        .unwrap()
        .eq_ignore_ascii_case("upgrade"));
    assert!(seen.contains_key("sec-websocket-key"));
    assert_eq!(seen["sec-websocket-version"], "13");
    assert_eq!(seen["x-forwarded-for"], "127.0.0.1");
    assert_eq!(seen["x-forwarded-host"], HOST);
    assert_eq!(seen["x-request-id"].len(), 32);
}

#[tokio::test]
async fn many_websockets_work_at_once() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut sockets = Vec::new();
    for _ in 0..5 {
        sockets.push(connect(&proxy).await);
    }
    for (n, socket) in sockets.iter_mut().enumerate() {
        socket
            .send(Message::text(format!("from {n}")))
            .await
            .unwrap();
    }
    for (n, socket) in sockets.iter_mut().enumerate() {
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::text(format!("from {n}"))
        );
    }
}

#[tokio::test]
async fn an_open_tunnel_counts_as_a_connection_until_it_closes() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;
    socket.send(Message::text("x")).await.unwrap();
    socket.next().await.unwrap().unwrap();
    assert_eq!(proxy.plane.active_connections(), 1);

    socket.close(None).await.unwrap();
    drop(socket);
    assert!(eventually(|| proxy.plane.active_connections() == 0).await);
}

#[tokio::test]
async fn a_closing_upstream_ends_the_clients_websocket() {
    let server = WsEchoServer::start().await;
    let proxy = proxy_for(server.addr).await;
    let mut socket = connect(&proxy).await;
    socket.send(Message::Close(None)).await.unwrap();
    let ended = timeout(Duration::from_secs(2), async {
        while let Some(Ok(message)) = socket.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await;
    assert!(ended.is_ok(), "the close should be relayed back");
}

#[tokio::test]
async fn an_upstream_that_declines_the_upgrade_is_proxied_as_a_normal_response() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "not a websocket",
    })
    .await;
    let proxy = proxy_for(upstream.addr).await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "GET /chat HTTP/1.1\r\nHost: {HOST}\r\nConnection: Upgrade, close\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "not a websocket");
}

#[tokio::test]
async fn a_stray_upgrade_header_without_connection_upgrade_is_not_forwarded() {
    let upstream = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "ok",
    })
    .await;
    let proxy = proxy_for(upstream.addr).await;
    let reply = send_raw(
        proxy.addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {HOST}\r\nUpgrade: websocket\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(reply.status, 200);
    let seen = upstream.last_headers();
    assert!(!seen.contains_key("upgrade"));
}
