//! Maintenance through the real proxy and a real upstream: each scope, their
//! precedence, the response, bypass rules, expiry and the metrics, and that a
//! request answered by maintenance never reaches an endpoint.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_auth::GeneratedSecret;
use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    BodyContentType, BypassHeader, BypassRules, Cidr, Config, CustomBody, MaintenanceSettings,
    MaintenanceTarget, RouteId, SubsystemId,
};
use fast_proxy_routing::{compile_all, MaintenanceInputs, TlsInputs};

use crate::test_support::{eventually, get, route, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const SHOP: &str = "shop.example.com";
const BLOG: &str = "blog.example.com";

fn ok() -> Behavior {
    Behavior::Respond {
        status: 200,
        headers: vec![],
        body: "from the upstream",
    }
}

fn on(message: &str) -> MaintenanceSettings {
    MaintenanceSettings {
        message: message.into(),
        body: None,
        retry_after_secs: None,
        bypass: BypassRules::default(),
        expires_at_ms: None,
        set_by: "ops".into(),
        set_at_ms: 0,
    }
}

fn in_future(ms: i64) -> i64 {
    now_ms() + ms
}

fn now_ms() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap()
}

/// Two subsystems in front of two upstreams; "shop" has routes `/` and `/api`.
struct Rig {
    proxy: Harness,
    shop: Arc<TestUpstream>,
    blog: Arc<TestUpstream>,
    config: Config,
}

impl Rig {
    async fn start(trusted: &[&str]) -> Self {
        let shop = TestUpstream::start(ok()).await;
        let blog = TestUpstream::start(ok()).await;
        let config = Config {
            subsystems: vec![
                site_to(
                    1,
                    "shop",
                    SHOP,
                    shop.addr,
                    vec![route(10, "/"), route(11, "/api")],
                ),
                site_to(2, "blog", BLOG, blog.addr, vec![route(20, "/")]),
            ],
        };
        let proxy = Harness::proxying(config.clone(), trusted).await;
        Self {
            proxy,
            shop,
            blog,
            config,
        }
    }

    /// Makes `scopes` live, as a config swap does.
    fn set(&self, scopes: Vec<(MaintenanceTarget, MaintenanceSettings)>) {
        let snapshot = compile_all(
            2,
            &self.config,
            &self.proxy.holder.load().runtime_registry(),
            &TlsInputs::off(),
            &MaintenanceInputs {
                scopes,
                now_ms: now_ms(),
            },
        )
        .unwrap();
        self.proxy.holder.swap(snapshot);
    }

    async fn get(&self, host: &str, path: &str, extra: &str) -> crate::test_support::Reply {
        get(self.proxy.addr, host, path, extra).await
    }
}

fn cidr(raw: &str) -> Cidr {
    Cidr::parse("bypass.cidrs", raw).unwrap()
}

// ---- D1: the scopes ----

#[tokio::test]
async fn the_global_scope_answers_every_known_host_with_503_and_leaves_unknown_hosts_a_404() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(MaintenanceTarget::Global, on("deploying"))]);
    for host in [SHOP, BLOG] {
        let reply = rig.get(host, "/", "").await;
        assert_eq!(reply.status, 503, "{host}");
        assert!(reply.body.contains("deploying"));
    }
    assert_eq!(rig.get("nope.example.com", "/", "").await.status, 404);
    assert_eq!(rig.shop.request_count() + rig.blog.request_count(), 0);
}

#[tokio::test]
async fn a_subsystem_scope_answers_only_that_subsystem() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(
        MaintenanceTarget::Subsystem(SubsystemId(1)),
        on("shop only"),
    )]);
    assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
    assert_eq!(rig.get(SHOP, "/api/x", "").await.status, 503);
    let blog = rig.get(BLOG, "/", "").await;
    assert_eq!(blog.status, 200);
    assert_eq!(blog.body, "from the upstream");
}

#[tokio::test]
async fn a_route_scope_answers_only_that_route() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(
        MaintenanceTarget::Route(RouteId(11)),
        on("api down"),
    )]);
    assert_eq!(rig.get(SHOP, "/api/orders", "").await.status, 503);
    assert_eq!(rig.get(SHOP, "/index.html", "").await.status, 200);
    assert_eq!(rig.get(BLOG, "/api", "").await.status, 200);
}

#[tokio::test]
async fn the_widest_scope_answers_first() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![
        (MaintenanceTarget::Global, on("global words")),
        (
            MaintenanceTarget::Subsystem(SubsystemId(1)),
            on("subsystem words"),
        ),
        (MaintenanceTarget::Route(RouteId(11)), on("route words")),
    ]);
    let reply = rig.get(SHOP, "/api", "").await;
    assert!(reply.body.contains("global words"), "{}", reply.body);
    let text = rig.proxy.metrics.render();
    assert!(
        text.contains(r#"scope="global",subsystem="shop""#),
        "{text}"
    );
    assert!(!text.contains(r#"scope="subsystem""#));
}

#[tokio::test]
async fn a_request_answered_by_maintenance_touches_no_endpoint() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(MaintenanceTarget::Global, on("x"))]);
    for _ in 0..5 {
        assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
    }
    assert_eq!(rig.shop.request_count(), 0);
    assert_eq!(
        rig.shop
            .connections
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let text = rig.proxy.metrics.render();
    assert!(
        !text.contains("fastproxy_upstream_requests_total{"),
        "no endpoint series appeared: {text}"
    );
}

#[tokio::test]
async fn turning_maintenance_off_serves_again_with_the_next_request() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(MaintenanceTarget::Global, on("x"))]);
    assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
    rig.set(vec![]);
    assert_eq!(rig.get(SHOP, "/", "").await.status, 200);
}

#[tokio::test]
async fn a_scope_expires_by_the_clock_with_no_swap() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("brief");
    settings.expires_at_ms = Some(in_future(300));
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if rig.get(SHOP, "/", "").await.status == 200 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "requests are served again once the expiry passes"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn the_health_of_the_other_subsystem_is_untouched_by_a_scope_elsewhere() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(
        MaintenanceTarget::Subsystem(SubsystemId(1)),
        on("x"),
    )]);
    for _ in 0..3 {
        assert_eq!(rig.get(BLOG, "/", "").await.status, 200);
    }
    assert_eq!(rig.blog.request_count(), 3);
    assert!(eventually(|| rig.shop.request_count() == 0).await);
}

// ---- D2: the response ----

#[tokio::test]
async fn the_response_is_a_503_with_retry_after_no_store_and_a_request_id() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("back soon");
    settings.retry_after_secs = Some(90);
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let reply = rig.get(SHOP, "/", "").await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.header("retry-after"), Some("90"));
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert!(reply.header("x-request-id").is_some());
    assert!(reply
        .header("content-type")
        .unwrap()
        .starts_with("text/plain"));
    assert_eq!(reply.body, "back soon\n");
}

#[tokio::test]
async fn retry_after_is_derived_from_the_ttl_when_not_configured() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("x");
    settings.expires_at_ms = Some(in_future(600_000));
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let reply = rig.get(SHOP, "/", "").await;
    let seconds: u32 = reply.header("retry-after").unwrap().parse().unwrap();
    assert!((595..=600).contains(&seconds), "{seconds}");
    let plain = Rig::start(&[]).await;
    plain.set(vec![(MaintenanceTarget::Global, on("x"))]);
    assert_eq!(
        plain.get(SHOP, "/", "").await.header("retry-after"),
        Some("120")
    );
}

#[tokio::test]
async fn the_format_follows_the_accept_header() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(MaintenanceTarget::Global, on("back soon"))]);
    let json = rig.get(SHOP, "/", "Accept: application/json\r\n").await;
    assert_eq!(json.header("content-type"), Some("application/json"));
    assert!(json.body.contains(r#""code":"maintenance""#));
    assert!(json.body.contains(r#""message":"back soon""#));
    let html = rig.get(SHOP, "/", "Accept: text/html,*/*\r\n").await;
    assert_eq!(html.header("content-type"), Some("text/html"));
    assert!(html.body.contains("<h1>Down for maintenance</h1>"));
}

#[tokio::test]
async fn a_message_with_markup_cannot_inject_into_the_page() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(
        MaintenanceTarget::Global,
        on("<script>alert(1)</script>"),
    )]);
    let html = rig.get(SHOP, "/", "Accept: text/html\r\n").await;
    assert!(!html.body.contains("<script>"));
    assert!(html.body.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn a_custom_body_and_content_type_replace_the_built_in_page() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("ignored");
    settings.body = Some(CustomBody {
        content_type: BodyContentType::Json,
        text: r#"{"down":true}"#.into(),
    });
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let reply = rig.get(SHOP, "/", "Accept: text/html\r\n").await;
    assert_eq!(reply.status, 503);
    assert_eq!(reply.header("content-type"), Some("application/json"));
    assert_eq!(reply.body, r#"{"down":true}"#);
}

#[tokio::test]
async fn the_access_log_names_the_scope_that_answered() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("access.log");
    let upstream = TestUpstream::start(ok()).await;
    let config = Config {
        subsystems: vec![site_to(
            1,
            "shop",
            SHOP,
            upstream.addr,
            vec![route(10, "/")],
        )],
    };
    let proxy = Harness::proxying_with_log(config.clone(), &[], Some(&log)).await;
    proxy.holder.swap(
        compile_all(
            2,
            &config,
            &RuntimeRegistry::new(),
            &TlsInputs::off(),
            &MaintenanceInputs {
                scopes: vec![(MaintenanceTarget::Subsystem(SubsystemId(1)), on("x"))],
                now_ms: now_ms(),
            },
        )
        .unwrap(),
    );
    assert_eq!(get(proxy.addr, SHOP, "/", "").await.status, 503);
    proxy.access_log.flush().await;
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains(" 503 "), "{text}");
    assert!(text.contains("maintenance=subsystem"), "{text}");
}

// ---- D3: bypass ----

#[tokio::test]
async fn a_listed_network_is_served_and_told_it_passed() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("x");
    settings.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let reply = rig.get(SHOP, "/", "").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "from the upstream");
    assert_eq!(reply.header("x-maintenance-bypass"), Some("global"));
    assert_eq!(rig.shop.request_count(), 1);
    assert!(rig
        .proxy
        .metrics
        .render()
        .contains(r#"fastproxy_maintenance_bypass_total{scope="global",subsystem="shop"} 1"#));
}

#[tokio::test]
async fn a_network_that_is_not_listed_is_answered() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("x");
    settings.bypass.cidrs = vec![cidr("10.0.0.0/8")];
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
}

#[tokio::test]
async fn the_secret_header_lets_a_request_through_and_a_wrong_one_does_not() {
    let rig = Rig::start(&[]).await;
    let secret = GeneratedSecret::generate().unwrap();
    let mut settings = on("x");
    settings.bypass.header = Some(BypassHeader {
        name: "X-Maintenance-Pass".into(),
        secret_hash: secret.hash().clone(),
    });
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let right = format!("X-Maintenance-Pass: {}\r\n", secret.value());
    let reply = rig.get(SHOP, "/", &right).await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("x-maintenance-bypass"), Some("global"));
    assert_eq!(
        rig.get(SHOP, "/", "X-Maintenance-Pass: nope\r\n")
            .await
            .status,
        503
    );
    assert_eq!(rig.get(SHOP, "/", "").await.status, 503);
}

#[tokio::test]
async fn the_bypass_secret_never_reaches_the_upstream() {
    let rig = Rig::start(&[]).await;
    let secret = GeneratedSecret::generate().unwrap();
    let mut settings = on("x");
    settings.bypass.header = Some(BypassHeader {
        name: "X-Maintenance-Pass".into(),
        secret_hash: secret.hash().clone(),
    });
    // A network that lets this client through too, so the header is not what decides.
    settings.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    rig.set(vec![(MaintenanceTarget::Global, settings)]);

    let right = format!("X-Maintenance-Pass: {}\r\nX-Keep: yes\r\n", secret.value());
    for extra in [
        right.as_str(),
        "X-Maintenance-Pass: a-wrong-guess\r\nX-Keep: yes\r\n",
    ] {
        assert_eq!(rig.get(SHOP, "/", extra).await.status, 200);
        let seen = rig.shop.last_headers();
        assert!(
            seen.get("x-maintenance-pass").is_none(),
            "the bypass header reached the upstream: {seen:?}"
        );
        assert_eq!(
            seen.get("x-keep").unwrap(),
            "yes",
            "other headers are untouched"
        );
    }
}

#[tokio::test]
async fn a_forged_forwarded_for_from_an_untrusted_peer_does_not_bypass() {
    let rig = Rig::start(&[]).await;
    let mut settings = on("x");
    settings.bypass.cidrs = vec![cidr("10.0.0.0/8")];
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let reply = rig.get(SHOP, "/", "X-Forwarded-For: 10.1.2.3\r\n").await;
    assert_eq!(
        reply.status, 503,
        "the peer is 127.0.0.1 and nobody trusted it"
    );
}

#[tokio::test]
async fn a_trusted_proxys_forwarded_client_is_honoured() {
    let rig = Rig::start(&["127.0.0.1/32"]).await;
    let mut settings = on("x");
    settings.bypass.cidrs = vec![cidr("10.0.0.0/8")];
    rig.set(vec![(MaintenanceTarget::Global, settings)]);
    let inside = rig.get(SHOP, "/", "X-Forwarded-For: 10.1.2.3\r\n").await;
    assert_eq!(inside.status, 200);
    let outside = rig.get(SHOP, "/", "X-Forwarded-For: 203.0.113.9\r\n").await;
    assert_eq!(outside.status, 503);
    let spoofed = rig
        .get(SHOP, "/", "X-Forwarded-For: 10.1.2.3, 203.0.113.9\r\n")
        .await;
    assert_eq!(
        spoofed.status, 503,
        "the entry the proxy appended wins over what the client wrote"
    );
}

#[tokio::test]
async fn bypassing_a_route_does_not_bypass_a_subsystem_wide_scope() {
    let rig = Rig::start(&[]).await;
    let mut route_scope = on("route");
    route_scope.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    rig.set(vec![
        (
            MaintenanceTarget::Subsystem(SubsystemId(1)),
            on("whole subsystem"),
        ),
        (MaintenanceTarget::Route(RouteId(11)), route_scope),
    ]);
    let reply = rig.get(SHOP, "/api", "").await;
    assert_eq!(reply.status, 503);
    assert!(reply.body.contains("whole subsystem"));
}

#[tokio::test]
async fn passing_one_scope_does_not_pass_the_next() {
    let rig = Rig::start(&[]).await;
    let mut global = on("global");
    global.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    rig.set(vec![
        (MaintenanceTarget::Global, global),
        (
            MaintenanceTarget::Subsystem(SubsystemId(1)),
            on("subsystem"),
        ),
    ]);
    let reply = rig.get(SHOP, "/", "").await;
    assert_eq!(reply.status, 503);
    assert!(reply.body.contains("subsystem"));
    assert_eq!(
        rig.get(BLOG, "/", "").await.header("x-maintenance-bypass"),
        Some("global")
    );
}

#[tokio::test]
async fn a_request_passing_two_scopes_names_both() {
    let rig = Rig::start(&[]).await;
    let mut global = on("g");
    global.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    let mut route_scope = on("r");
    route_scope.bypass.cidrs = vec![cidr("127.0.0.0/8")];
    rig.set(vec![
        (MaintenanceTarget::Global, global),
        (MaintenanceTarget::Route(RouteId(11)), route_scope),
    ]);
    let reply = rig.get(SHOP, "/api", "").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("x-maintenance-bypass"), Some("global,route"));
}

// ---- D4: metrics ----

#[tokio::test]
async fn the_metrics_count_answers_and_the_dashboard_error_rate_stays_zero() {
    let rig = Rig::start(&[]).await;
    rig.set(vec![(MaintenanceTarget::Route(RouteId(11)), on("x"))]);
    for _ in 0..4 {
        assert_eq!(rig.get(SHOP, "/api", "").await.status, 503);
    }
    assert!(
        eventually(|| {
            rig.proxy.metrics.render().contains(
        r#"fastproxy_maintenance_responses_total{route="/api",scope="route",subsystem="shop"} 4"#
    )
        })
        .await
    );
    assert!(eventually(|| rig.proxy.metrics.traffic_totals().overall().requests == 4).await);
    assert_eq!(rig.proxy.metrics.traffic_totals().overall().errors, 0);
}

// ---- upgrades ----

#[tokio::test]
async fn a_websocket_handshake_gets_the_503_while_an_open_tunnel_survives() {
    use crate::test_upstream::WsEchoServer;
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpStream;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::{client_async, MaybeTlsStream};

    let server = WsEchoServer::start().await;
    let config = Config {
        subsystems: vec![site_to(1, "shop", SHOP, server.addr, vec![route(10, "/")])],
    };
    let proxy = Harness::proxying(config.clone(), &[]).await;

    let connect = || async {
        let stream = TcpStream::connect(proxy.addr).await.unwrap();
        let request = format!("ws://{SHOP}/chat").into_client_request().unwrap();
        client_async(request, MaybeTlsStream::Plain(stream)).await
    };
    let (mut open, response) = connect().await.expect("a handshake before maintenance");
    assert_eq!(response.status(), 101);

    proxy.holder.swap(
        compile_all(
            2,
            &config,
            &proxy.holder.load().runtime_registry(),
            &TlsInputs::off(),
            &MaintenanceInputs {
                scopes: vec![(MaintenanceTarget::Global, on("x"))],
                now_ms: now_ms(),
            },
        )
        .unwrap(),
    );

    assert!(connect().await.is_err(), "a new handshake is refused");
    open.send(Message::text("still here")).await.unwrap();
    assert_eq!(
        open.next().await.unwrap().unwrap(),
        Message::text("still here"),
        "the established tunnel keeps working"
    );
}
