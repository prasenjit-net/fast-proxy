//! HTTP/2 to clients, the 421 rules and handshake hygiene (M3), with real TLS
//! connections to the real data plane.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use common::tls_stack::{Options, Stack};
use fast_proxy_dataplane::Http2Limits;
use fast_proxy_model::HttpsPolicy;
use fast_proxy_test_pki::{connect, ClientHello, LeafSpec, TestCa};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http2::SendRequest;
use hyper::{Request, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type Client = SendRequest<Empty<Bytes>>;

/// An HTTP/2 connection to the HTTPS listener, negotiated by ALPN.
async fn h2(stack: &Stack, sni: Option<&str>, ca: &TestCa) -> Client {
    let hello = ClientHello {
        sni: sni.map(str::to_string),
        trust_pem: vec![ca.pem.clone()],
        alpn: vec!["h2".into()],
        insecure: sni.is_none(),
        ..ClientHello::default()
    };
    let (tls, negotiated) = connect(stack.https, &hello).await.unwrap();
    assert_eq!(
        negotiated.alpn.as_deref(),
        Some(&b"h2"[..]),
        "ALPN chose h2"
    );
    let (send, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(connection);
    send
}

fn get(authority: &str, path: &str) -> Request<Empty<Bytes>> {
    Request::builder()
        .uri(format!("https://{authority}{path}"))
        .body(Empty::new())
        .unwrap()
}

async fn fetch(client: &mut Client, authority: &str, path: &str) -> (StatusCode, String) {
    let response = client.send_request(get(authority, path)).await.unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).into_owned())
}

/// A backend that holds each request for `delay` and records how many it held
/// at once (`peak`) and right now (`current`).
async fn slow_backend(delay: Duration) -> (SocketAddr, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (peak, current) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (peak_seen, current_seen) = (peak.clone(), current.clone());
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let (peak, current) = (peak_seen.clone(), current_seen.clone());
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(delay).await;
                current.fetch_sub(1, Ordering::SeqCst);
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    (addr, peak, current)
}

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never happened: {what}");
}

async fn shop(stack: &Stack, ca: &TestCa) {
    let id = stack.add_site("shop", "shop.example.com").await;
    stack
        .serve_with(id, "shop", ca, &["shop.example.com"])
        .await;
}

#[tokio::test]
async fn http2_and_http11_are_negotiated_on_one_port_and_proxied_to_an_http11_backend() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;

    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    let (status, body) = fetch(&mut client, "shop.example.com", "/cart?x=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("get /cart?x=1 http/1.1"),
        "the backend speaks HTTP/1.1: {body}"
    );
    assert!(
        body.contains("host: shop.example.com"),
        "the :authority became Host: {body}"
    );
    assert!(body.contains("x-forwarded-proto: https"), "{body}");
    assert!(
        body.contains("x-forwarded-host: shop.example.com"),
        "{body}"
    );

    let hello = ClientHello {
        sni: Some("shop.example.com".into()),
        trust_pem: vec![ca.pem.clone()],
        alpn: vec!["http/1.1".into()],
        ..ClientHello::default()
    };
    let answer = fast_proxy_test_pki::request(
        stack.https,
        &hello,
        b"GET /cart HTTP/1.1\r\nHost: shop.example.com\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
}

#[tokio::test]
async fn http2_can_be_turned_off_so_clients_fall_back_to_http11() {
    let stack = Stack::start(Options {
        offer_http2: false,
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let hello = ClientHello {
        sni: Some("shop.example.com".into()),
        trust_pem: vec![ca.pem.clone()],
        alpn: vec!["h2".into(), "http/1.1".into()],
        ..ClientHello::default()
    };
    let (_, negotiated) = connect(stack.https, &hello).await.unwrap();
    assert_eq!(negotiated.alpn.as_deref(), Some(&b"http/1.1"[..]));
}

#[tokio::test]
async fn many_requests_are_multiplexed_on_one_connection() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let client = h2(&stack, Some("shop.example.com"), &ca).await;
    let mut tasks = Vec::new();
    for index in 0..20 {
        let mut client = client.clone();
        tasks.push(tokio::spawn(async move {
            fetch(&mut client, "shop.example.com", &format!("/item/{index}")).await
        }));
    }
    for (index, task) in tasks.into_iter().enumerate() {
        let (status, body) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.contains(&format!("get /item/{index} http/1.1")),
            "{body}"
        );
    }
    let totals = stack.metrics.traffic_totals();
    assert_eq!(
        totals.active_connections, 1,
        "one connection, not one per stream"
    );
}

#[tokio::test]
async fn the_stream_limit_is_enforced() {
    let (backend, peak, _) = slow_backend(Duration::from_millis(150)).await;
    let stack = Stack::start_with_backend(
        Options {
            http2: Http2Limits {
                max_concurrent_streams: 2,
                ..Http2Limits::default()
            },
            ..Options::default()
        },
        Some(backend),
    )
    .await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    // One request first, so the server's SETTINGS (the limit) have been received.
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );

    // A client that honours the advertised limit is served in full, two at a time.
    let mut polite = Vec::new();
    for _ in 0..8 {
        let mut client = client.clone();
        polite.push(tokio::spawn(async move {
            fetch(&mut client, "shop.example.com", "/").await.0
        }));
    }
    for task in polite {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert!(
        peak.load(Ordering::SeqCst) <= 2,
        "at most 2 streams at a time, saw {}",
        peak.load(Ordering::SeqCst)
    );
    assert!(
        peak.load(Ordering::SeqCst) >= 1,
        "the backend saw the requests"
    );
}

#[tokio::test]
async fn an_oversized_header_list_is_refused() {
    let stack = Stack::start(Options {
        http2: Http2Limits {
            max_header_list_size: 4096,
            ..Http2Limits::default()
        },
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    let request = Request::builder()
        .uri("https://shop.example.com/")
        .header("x-big", "a".repeat(64 * 1024))
        .body(Empty::<Bytes>::new())
        .unwrap();
    // Either the stream is answered with 431 or it is refused outright; it is
    // never served.
    if let Ok(response) = client.send_request(request).await {
        assert_eq!(
            response.status(),
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
        );
    }
    let mut fresh = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut fresh, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_rapid_reset_attack_is_contained_and_in_flight_counts_return_to_zero() {
    let (backend, _, _) = slow_backend(Duration::from_millis(200)).await;
    let stack = Stack::start_with_backend(Options::default(), Some(backend)).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut attacker = h2(&stack, Some("shop.example.com"), &ca).await;
    for _ in 0..300 {
        if attacker.ready().await.is_err() {
            break; // the server closed the abusive connection
        }
        // Open a stream and cancel it at once by dropping the response future.
        drop(attacker.send_request(get("shop.example.com", "/")));
    }
    let mut honest = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut honest, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    drop(attacker);
    drop(honest);
    wait_until("no request is left in flight", || {
        stack.metrics.traffic_totals().active_requests == 0
    })
    .await;
}

#[tokio::test]
async fn a_drain_lets_open_streams_finish_before_the_connection_closes() {
    let (backend, _, current) = slow_backend(Duration::from_millis(400)).await;
    let mut stack = Stack::start_with_backend(Options::default(), Some(backend)).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    let in_flight = tokio::spawn(async move { fetch(&mut client, "shop.example.com", "/").await });
    wait_until("the request reached the backend", || {
        current.load(Ordering::SeqCst) == 1
    })
    .await;

    let report = stack.drain(Duration::from_secs(10)).await;
    let (status, body) = in_flight.await.unwrap();
    assert_eq!(status, StatusCode::OK, "the open stream finished");
    assert_eq!(body, "ok");
    assert_eq!(report.forced_connections, 0);
}

// ---- the 421 rules ----------------------------------------------------------------

#[tokio::test]
async fn hosts_sharing_one_certificate_may_share_a_connection() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let shop = stack.add_site("shop", "shop.example.com").await;
    let blog = stack.add_site("blog", "blog.example.com").await;
    let id = stack
        .serve_with(shop, "wild", &ca, &["*.example.com"])
        .await;
    let https = HttpsPolicy {
        enabled: true,
        certificate_id: Some(id),
        ..HttpsPolicy::default()
    };
    stack
        .service
        .set_subsystem_https(blog, https)
        .await
        .unwrap();
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    assert_eq!(
        fetch(&mut client, "blog.example.com", "/").await.0,
        StatusCode::OK,
        "coalescing works"
    );
}

#[tokio::test]
async fn a_host_served_by_a_different_certificate_gets_421() {
    let stack = Stack::start(Options::default()).await;
    let (shop_ca, blog_ca) = (TestCa::new("shop ca"), TestCa::new("blog ca"));
    let shop = stack.add_site("shop", "shop.example.com").await;
    let blog = stack.add_site("blog", "blog.example.com").await;
    stack
        .serve_with(shop, "shop", &shop_ca, &["shop.example.com"])
        .await;
    stack
        .serve_with(blog, "blog", &blog_ca, &["blog.example.com"])
        .await;
    let mut client = h2(&stack, Some("shop.example.com"), &shop_ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    let (status, _) = fetch(&mut client, "blog.example.com", "/").await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK,
        "the connection stays usable"
    );
    let (status, _) = fetch(&mut client, "nobody.example.com", "/").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unknown host is a 404, not a 421"
    );
}

#[tokio::test]
async fn a_host_of_a_subsystem_without_https_gets_421() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    stack.add_site("plain", "plain.example.com").await;
    let insecure = ClientHello {
        sni: Some("plain.example.com".into()),
        insecure: true,
        alpn: vec!["h2".into()],
        ..ClientHello::default()
    };
    let (tls, _) = connect(stack.https, &insecure).await.unwrap();
    let (mut plain, connection) =
        hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tls))
            .await
            .unwrap();
    tokio::spawn(connection);
    let (status, _) = fetch(&mut plain, "plain.example.com", "/").await;
    assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
}

#[tokio::test]
async fn a_client_without_sni_skips_the_certificate_rule() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut client = h2(&stack, None, &ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn replacing_a_certificate_mid_connection_does_not_misdirect_the_connection() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    let site = stack.add_site("shop", "shop.example.com").await;
    let id = stack
        .serve_with(site, "shop", &ca, &["shop.example.com"])
        .await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    let renewed = ca.issue(&LeafSpec::new(&["shop.example.com"]));
    stack
        .service
        .replace_certificate(
            id,
            fast_proxy_config_service::CertificateChange {
                cert_pem: Some(renewed.chain_pem),
                key_pem: Some(renewed.key_pem),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}

// ---- handshake hygiene --------------------------------------------------------------

#[tokio::test]
async fn stalled_handshakes_cannot_starve_an_established_connection() {
    let stack = Stack::start(Options {
        max_handshakes: 4,
        handshake_timeout: Duration::from_secs(2),
        ..Options::default()
    })
    .await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut established = h2(&stack, Some("shop.example.com"), &ca).await;

    let mut stalled = Vec::new();
    for _ in 0..4 {
        stalled.push(TcpStream::connect(stack.https).await.unwrap());
    }
    // Every permit is taken by a stalled handshake: a fifth is refused at once...
    let mut refused = TcpStream::connect(stack.https).await.unwrap();
    let mut sink = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(1), refused.read_to_end(&mut sink)).await;
    assert!(
        closed.is_ok(),
        "an extra handshake is refused rather than queued"
    );
    // ...while the established connection keeps its normal latency.
    let started = Instant::now();
    assert_eq!(
        fetch(&mut established, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "served in {:?}",
        started.elapsed()
    );

    // The stalled ones are cut off at the timeout and the permits come back.
    wait_until("a new handshake succeeds again", || {
        stack.metrics.render().contains("result=\"timeout\"")
    })
    .await;
    drop(stalled);
    let mut again = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut again, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn an_oversized_client_hello_and_a_flood_of_failing_handshakes_do_no_harm() {
    let stack = Stack::start(Options::default()).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut established = h2(&stack, Some("shop.example.com"), &ca).await;

    let mut flood = Vec::new();
    for _ in 0..50 {
        let address = stack.https;
        flood.push(tokio::spawn(async move {
            let mut stream = TcpStream::connect(address).await.unwrap();
            // A handshake record claiming 64 KiB, then a megabyte of junk.
            let mut hello = vec![0x16, 0x03, 0x01, 0xff, 0xff];
            hello.extend(std::iter::repeat_n(0u8, 1024 * 1024));
            let _ = stream.write_all(&hello).await;
            let mut sink = Vec::new();
            let _ = stream.read_to_end(&mut sink).await;
        }));
    }
    for task in flood {
        task.await.unwrap();
    }
    stack.metric("result=\"failed\"").await;
    assert_eq!(
        fetch(&mut established, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    let mut fresh = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut fresh, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}

// ---- more HTTP/2 behaviour (Q4) ---------------------------------------------------

/// A backend that reads the whole request body slowly (a pause between reads)
/// and answers with how many body bytes it got.
async fn slow_reader_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut received = Vec::new();
                let mut chunk = [0u8; 16 * 1024];
                let mut body_start = None;
                let mut length = 0usize;
                loop {
                    let read = stream.read(&mut chunk).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    received.extend_from_slice(&chunk[..read]);
                    if body_start.is_none() {
                        if let Some(end) = received.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&received[..end]).to_lowercase();
                            length = head
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length: "))
                                .and_then(|value| value.trim().parse().ok())
                                .unwrap_or(0);
                            body_start = Some(end + 4);
                        }
                    }
                    if let Some(start) = body_start {
                        if received.len() - start >= length {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                let reply = format!("got {length}");
                let _ = stream
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        )
                        .as_bytes(),
                    )
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn a_large_body_flows_through_a_slow_backend_with_flow_control() {
    let backend = slow_reader_backend().await;
    let stack = Stack::start_with_backend(Options::default(), Some(backend)).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let hello = ClientHello {
        sni: Some("shop.example.com".into()),
        trust_pem: vec![ca.pem.clone()],
        alpn: vec!["h2".into()],
        ..ClientHello::default()
    };
    let (tls, _) = connect(stack.https, &hello).await.unwrap();
    let (mut client, connection) = hyper::client::conn::http2::handshake::<
        _,
        _,
        http_body_util::Full<Bytes>,
    >(TokioExecutor::new(), TokioIo::new(tls))
    .await
    .unwrap();
    tokio::spawn(connection);
    let size = 4 * 1024 * 1024;
    let request = Request::builder()
        .method("POST")
        .uri("https://shop.example.com/upload")
        .header("content-length", size.to_string())
        .body(http_body_util::Full::new(Bytes::from(vec![7u8; size])))
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(60), client.send_request(request))
        .await
        .expect("the upload finishes")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body, format!("got {size}"));
}

#[tokio::test]
async fn many_streams_succeed_together_while_a_dead_endpoint_is_retried_around() {
    let (live, _, _) = slow_backend(Duration::from_millis(20)).await;
    let dead: SocketAddr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let stack = Stack::start_with_backend(Options::default(), Some(live)).await;
    let ca = TestCa::new("ca");
    let id = stack
        .add_site_to(
            "shop",
            "shop.example.com",
            &[dead, live],
            fast_proxy_model::UpstreamTls::default(),
        )
        .await;
    stack
        .serve_with(id, "shop", &ca, &["shop.example.com"])
        .await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
    let mut tasks = Vec::new();
    for _ in 0..60 {
        let mut client = client.clone();
        tasks.push(tokio::spawn(async move {
            fetch(&mut client, "shop.example.com", "/").await.0
        }));
    }
    for task in tasks {
        assert_eq!(
            task.await.unwrap(),
            StatusCode::OK,
            "retries hide the dead endpoint"
        );
    }
}

#[tokio::test]
async fn cancelling_one_stream_leaves_the_others_alone_and_in_flight_returns_to_zero() {
    let (backend, _, current) = slow_backend(Duration::from_millis(300)).await;
    let stack = Stack::start_with_backend(Options::default(), Some(backend)).await;
    let ca = TestCa::new("ca");
    shop(&stack, &ca).await;
    let mut client = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut client, "shop.example.com", "/").await.0,
        StatusCode::OK
    );

    let mut kept = Vec::new();
    for _ in 0..4 {
        let mut client = client.clone();
        kept.push(tokio::spawn(async move {
            fetch(&mut client, "shop.example.com", "/").await
        }));
    }
    let mut doomed = client.clone();
    let cancelled = tokio::spawn(async move { fetch(&mut doomed, "shop.example.com", "/").await });
    wait_until("all five reached the backend", || {
        current.load(Ordering::SeqCst) == 5
    })
    .await;
    cancelled.abort();

    for task in kept {
        let (status, body) = task.await.unwrap();
        assert_eq!((status, body.as_str()), (StatusCode::OK, "ok"));
    }
    wait_until("nothing is left in flight", || {
        stack.metrics.traffic_totals().active_requests == 0
    })
    .await;
    let mut fresh = h2(&stack, Some("shop.example.com"), &ca).await;
    assert_eq!(
        fetch(&mut fresh, "shop.example.com", "/").await.0,
        StatusCode::OK
    );
}
