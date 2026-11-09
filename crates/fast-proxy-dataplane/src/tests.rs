//! End-to-end tests of the listeners and request pipeline: a real server on
//! an ephemeral port, driven over raw TCP, with a fake forwarder standing in
//! for the upstream client (task D4).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use fast_proxy_model::{
    Config, EndpointAddr, EndpointConfig, HostPattern, RouteConfig, RouteId, RoutePrefix,
    SubsystemConfig, SubsystemId, SubsystemName, UpstreamGroupConfig, UpstreamGroupId,
    UpstreamPrefix,
};
use fast_proxy_routing::{compile, SnapshotHolder};
use http::{Request, Response};
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use crate::{bind, DataPlane, Forward, ListenerSpec, RequestPipeline, ResponseBody, Target};

/// Answers every routed request with a description of how it was routed.
struct EchoForward;

impl Forward for EchoForward {
    async fn forward<'a>(
        &'a self,
        request: Request<Incoming>,
        target: Target<'a>,
    ) -> Response<ResponseBody> {
        let rest = &request.uri().path()[target.consumed_prefix_len..];
        let body = format!(
            "matched {} route={} backend={} rest={rest} id={}",
            target.subsystem.name.as_str(),
            target.route.id.0,
            target.route.endpoint,
            target.request_id
        );
        Response::new(
            Full::new(Bytes::from(body))
                .map_err(|never| match never {})
                .boxed(),
        )
    }
}

fn site(id: i64, name: &str, host: &str, prefixes: &[&str], enabled: bool) -> SubsystemConfig {
    SubsystemConfig {
        id: SubsystemId(id),
        name: SubsystemName::parse(name).unwrap(),
        enabled,
        description: String::new(),
        hosts: vec![HostPattern::parse(host).unwrap()],
        upstream_groups: vec![UpstreamGroupConfig {
            id: UpstreamGroupId(id),
            name: "backend".into(),
            endpoints: vec![EndpointConfig {
                addr: EndpointAddr::parse(&format!("127.0.0.1:{}", 3000 + id)).unwrap(),
                weight: 1,
            }],
        }],
        routes: prefixes
            .iter()
            .enumerate()
            .map(|(index, prefix)| RouteConfig {
                id: RouteId(id * 100 + index as i64),
                prefix: RoutePrefix::parse(prefix).unwrap(),
                upstream_group_id: UpstreamGroupId(id),
                strip_prefix: false,
                upstream_prefix: UpstreamPrefix::default(),
                preserve_host: true,
                connect_timeout: Duration::from_secs(5),
                response_timeout: Duration::from_secs(30),
                enabled: true,
            })
            .collect(),
    }
}

fn config() -> Config {
    Config {
        subsystems: vec![
            site(1, "shop", "shop.example.com", &["/", "/api"], true),
            site(2, "blog", "blog.example.com", &["/posts"], true),
            site(3, "off", "off.example.com", &["/"], false),
        ],
    }
}

struct Harness {
    plane: DataPlane,
    holder: Arc<SnapshotHolder>,
    addr: SocketAddr,
    shutdown: CancellationToken,
}

async fn start() -> Harness {
    let holder = Arc::new(SnapshotHolder::new());
    holder.swap(compile(1, &config()).unwrap());
    let specs = [ListenerSpec {
        name: "http".into(),
        addr: ([127, 0, 0, 1], 0).into(),
    }];
    let bound = bind(&specs).await.unwrap();
    let shutdown = CancellationToken::new();
    let pipeline = Arc::new(RequestPipeline::new(holder.clone(), EchoForward));
    let plane = DataPlane::start(bound, pipeline, shutdown.clone());
    let addr = plane.local_addrs()[0].1;
    Harness {
        plane,
        holder,
        addr,
        shutdown,
    }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// Sends `raw` and reads until the server closes the connection.
async fn send_raw(addr: SocketAddr, raw: &str) -> Reply {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).await.unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers = lines
        .map(|line| {
            let (key, value) = line.split_once(':').unwrap();
            (key.to_string(), value.trim().to_string())
        })
        .collect();
    Reply {
        status,
        headers,
        body: body.to_string(),
    }
}

async fn get(addr: SocketAddr, host: &str, path: &str, extra: &str) -> Reply {
    send_raw(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Connection: close\r\n\r\n"),
    )
    .await
}

async fn eventually(mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn a_bound_port_serves_a_routed_response() {
    let harness = start().await;
    let reply = get(harness.addr, "shop.example.com", "/hello", "").await;
    assert_eq!(reply.status, 200);
    assert!(reply
        .body
        .starts_with("matched shop route=100 backend=127.0.0.1:3001 rest=/hello"));
}

#[tokio::test]
async fn routes_by_host_and_longest_prefix() {
    let harness = start().await;
    let api = get(harness.addr, "shop.example.com", "/api/users", "").await;
    assert!(api.body.contains("route=101"), "{}", api.body);
    assert!(api.body.contains("rest=/users"), "{}", api.body);
    let blog = get(harness.addr, "BLOG.example.com:8000", "/posts/1", "").await;
    assert!(
        blog.body
            .starts_with("matched blog route=200 backend=127.0.0.1:3002"),
        "{}",
        blog.body
    );
}

#[tokio::test]
async fn an_unknown_host_is_404() {
    let harness = start().await;
    let reply = get(harness.addr, "nope.example.com", "/", "").await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.body, "unknown host\n");
    assert_eq!(
        reply.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
}

#[tokio::test]
async fn a_missing_host_header_is_an_unknown_host() {
    let harness = start().await;
    let reply = send_raw(harness.addr, "GET / HTTP/1.0\r\n\r\n").await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.body, "unknown host\n");
}

#[tokio::test]
async fn the_host_from_an_absolute_form_uri_is_used_when_there_is_no_host_header() {
    let harness = start().await;
    let reply = send_raw(
        harness.addr,
        "GET http://shop.example.com/x HTTP/1.0\r\n\r\n",
    )
    .await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains("matched shop"), "{}", reply.body);
}

#[tokio::test]
async fn a_disabled_subsystem_is_503() {
    let harness = start().await;
    let reply = get(harness.addr, "off.example.com", "/", "").await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.body, "service unavailable\n");
}

#[tokio::test]
async fn a_path_without_a_route_is_404() {
    let harness = start().await;
    let reply = get(harness.addr, "blog.example.com", "/other", "").await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.body, "no route matches this path\n");
}

#[tokio::test]
async fn error_responses_are_json_when_the_client_accepts_json() {
    let harness = start().await;
    let reply = get(
        harness.addr,
        "nope.example.com",
        "/",
        "Accept: application/json\r\n",
    )
    .await;
    assert_eq!(reply.status, 404);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    let id = reply.header("x-request-id").unwrap();
    assert_eq!(
        reply.body,
        format!(
            r#"{{"error":{{"code":"unknown_host","message":"unknown host","requestId":"{id}"}}}}"#
        )
    );
}

#[tokio::test]
async fn every_response_carries_a_request_id() {
    let harness = start().await;
    let cases = [
        ("shop.example.com", "/"),
        ("nope.example.com", "/"),
        ("off.example.com", "/"),
        ("blog.example.com", "/nope"),
    ];
    for (host, path) in cases {
        let reply = get(harness.addr, host, path, "").await;
        let id = reply.header("x-request-id").unwrap_or_default();
        assert_eq!(id.len(), 32, "{host}{path}");
    }
}

#[tokio::test]
async fn a_client_request_id_is_kept_and_a_bad_one_is_replaced() {
    let harness = start().await;
    let kept = get(
        harness.addr,
        "shop.example.com",
        "/",
        "X-Request-Id: trace-42\r\n",
    )
    .await;
    assert_eq!(kept.header("x-request-id"), Some("trace-42"));
    assert!(kept.body.ends_with("id=trace-42"), "{}", kept.body);

    let long = "a".repeat(200);
    let replaced = get(
        harness.addr,
        "nope.example.com",
        "/",
        &format!("X-Request-Id: {long}\r\n"),
    )
    .await;
    assert_eq!(replaced.header("x-request-id").unwrap().len(), 32);
}

#[tokio::test]
async fn a_config_swap_takes_effect_for_the_next_request() {
    let harness = start().await;
    assert_eq!(
        get(harness.addr, "new.example.com", "/", "").await.status,
        404
    );

    let mut updated = config();
    updated
        .subsystems
        .push(site(4, "fresh", "new.example.com", &["/"], true));
    harness.holder.swap(compile(2, &updated).unwrap());

    let reply = get(harness.addr, "new.example.com", "/", "").await;
    assert_eq!(reply.status, 200);
    assert!(reply.body.contains("matched fresh"), "{}", reply.body);
}

#[tokio::test]
async fn open_connections_are_counted() {
    let harness = start().await;
    assert_eq!(harness.plane.active_connections(), 0);
    let stream = TcpStream::connect(harness.addr).await.unwrap();
    assert!(eventually(|| harness.plane.active_connections() == 1).await);
    drop(stream);
    assert!(eventually(|| harness.plane.active_connections() == 0).await);
}

#[tokio::test]
async fn shutdown_stops_accepting_new_connections() {
    let harness = start().await;
    let addr = harness.addr;
    harness.shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), harness.plane.stopped())
        .await
        .expect("accept loops should stop");
    assert!(TcpStream::connect(addr).await.is_err());
}

#[tokio::test]
async fn binding_an_address_in_use_names_the_listener() {
    let taken = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = taken.local_addr().unwrap();
    let specs = [
        ListenerSpec {
            name: "free".into(),
            addr: ([127, 0, 0, 1], 0).into(),
        },
        ListenerSpec {
            name: "clash".into(),
            addr,
        },
    ];
    let err = bind(&specs).await.err().expect("bind must fail");
    assert_eq!(err.name, "clash");
    assert_eq!(err.addr, addr);
    assert!(err.to_string().contains("\"clash\""), "{err}");
}

#[tokio::test]
async fn several_listeners_all_serve_the_same_pipeline() {
    let holder = Arc::new(SnapshotHolder::new());
    holder.swap(compile(1, &config()).unwrap());
    let specs = ["a", "b"].map(|name| ListenerSpec {
        name: name.into(),
        addr: ([127, 0, 0, 1], 0).into(),
    });
    let bound = bind(&specs).await.unwrap();
    let pipeline = Arc::new(RequestPipeline::new(holder, EchoForward));
    let plane = DataPlane::start(bound, pipeline, CancellationToken::new());
    assert_eq!(plane.local_addrs().len(), 2);
    for (_, addr) in plane.local_addrs() {
        assert_eq!(get(*addr, "shop.example.com", "/", "").await.status, 200);
    }
}
