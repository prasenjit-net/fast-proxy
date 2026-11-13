//! Request sampling end to end: a real proxy, real backends (one killed, one
//! slow, one failing), a collecting sink in place of the database.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_model::{
    Config, MaintenanceSettings, MaintenanceTarget, SamplingOverride, SamplingSettings,
    SamplingTarget, SubsystemId,
};
use fast_proxy_routing::{
    compile_full, MaintenanceInputs, SamplingInputs, SamplingRuntime, TlsInputs,
};
use fast_proxy_sampling::{Redactor, Sample, SampleSink, SamplerConfig, SamplerRuntime, SinkError};
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use tokio_util::sync::CancellationToken;

use crate::test_support::{get, route, send_raw, site_to, site_to_group, Reply};
use crate::test_upstream::{Behavior, TestUpstream};
use crate::{
    bind, DataPlane, ListenerSpec, Observability, RequestPipeline, TrustedProxies, UpstreamClient,
};

const HOST: &str = "shop.example.com";
const SENTINEL_HOST: &str = "sentinel.example.com";
const PATIENCE: Duration = Duration::from_secs(20);

#[derive(Clone, Default)]
struct Collected(Arc<Mutex<Vec<Sample>>>);

impl SampleSink for Collected {
    fn write_batch(&mut self, batch: Vec<Sample>) -> Result<(), SinkError> {
        self.0.lock().unwrap().extend(batch);
        Ok(())
    }
}

/// The settings every test starts from: nothing but the trigger under test
/// can match, and the limits never get in the way.
fn quiet() -> SamplingSettings {
    SamplingSettings {
        baseline_per_mille: 0,
        rate_per_sec: 10_000,
        rate_burst: 10_000,
        storm_burst: 100,
        slow_threshold_ms: 3_600_000,
        client_errors_per_min: 1_000_000,
        ..SamplingSettings::default()
    }
}

struct Sampled {
    addr: std::net::SocketAddr,
    plane: Option<DataPlane>,
    shutdown: CancellationToken,
    runtime: Option<SamplerRuntime>,
    collected: Collected,
    metrics: Arc<Metrics>,
    holder: Arc<fast_proxy_routing::SnapshotHolder>,
    sentinel: Arc<TestUpstream>,
}

struct Setup {
    settings: SamplingSettings,
    overrides: Vec<(SamplingTarget, SamplingOverride)>,
    maintenance: MaintenanceInputs,
    body_budget_bytes: u64,
}

impl Setup {
    fn new(settings: SamplingSettings) -> Self {
        Self {
            settings,
            overrides: Vec::new(),
            maintenance: MaintenanceInputs::none(),
            body_budget_bytes: 1 << 20,
        }
    }
}

fn an_override() -> SamplingOverride {
    SamplingOverride {
        enabled: None,
        capture_bodies: None,
        baseline_per_mille: None,
        slow_threshold_ms: None,
        slow_p99_factor_x10: None,
        set_by: "test".into(),
        set_at_ms: 0,
    }
}

/// A proxy for the `shop` subsystem (id 1) with the given routes, plus a
/// `sentinel` subsystem (id 2) whose every request is sampled, so a test can
/// tell when everything before it has been decided.
async fn start(shop: fast_proxy_model::SubsystemConfig, mut setup: Setup) -> Sampled {
    let sentinel = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: Vec::new(),
        body: "sentinel",
    })
    .await;
    let mut sentinel_override = an_override();
    sentinel_override.baseline_per_mille = Some(1000);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(2)), sentinel_override));
    let config = Config {
        subsystems: vec![
            shop,
            site_to(
                2,
                "sentinel",
                SENTINEL_HOST,
                sentinel.addr,
                vec![route(9, "/")],
            ),
        ],
    };
    let holder = Arc::new(fast_proxy_routing::SnapshotHolder::new());
    holder.swap(
        compile_full(
            1,
            &config,
            &RuntimeRegistry::new(),
            &TlsInputs::off(),
            &setup.maintenance,
            &SamplingInputs {
                settings: setup.settings.clone(),
                overrides: setup.overrides,
                runtime: SamplingRuntime::new(),
            },
        )
        .unwrap(),
    );
    let collected = Collected::default();
    let runtime = SamplerRuntime::start(
        SamplerConfig {
            queue_depth: 256,
            batch_size: 1,
            batch_interval: Duration::from_millis(10),
            body_budget_bytes: setup.body_budget_bytes,
            instance: "test".into(),
        },
        Redactor::new(&setup.settings, &[]),
        collected.clone(),
    )
    .unwrap();
    let metrics = Arc::new(Metrics::new());
    let observability = Observability::new(metrics.clone(), Arc::new(AccessLog::open(None).await))
        .with_sampler(Arc::new(runtime.handle()));
    let bound = bind(&[ListenerSpec {
        name: "http".into(),
        addr: ([127, 0, 0, 1], 0).into(),
    }])
    .await
    .unwrap();
    let pipeline = Arc::new(
        RequestPipeline::new(
            holder.clone(),
            UpstreamClient::new(
                TrustedProxies::default(),
                metrics.clone(),
                EndpointEvents::new(),
            ),
            observability,
        )
        .with_trusted_proxies(TrustedProxies::default()),
    );
    let shutdown = CancellationToken::new();
    let plane = DataPlane::start(bound, pipeline, shutdown.clone());
    let addr = plane.local_addrs()[0].1;
    Sampled {
        addr,
        plane: Some(plane),
        shutdown,
        runtime: Some(runtime),
        collected,
        metrics,
        holder,
        sentinel,
    }
}

impl Sampled {
    fn all(&self) -> Vec<Sample> {
        self.collected.0.lock().unwrap().clone()
    }

    fn shop(&self) -> Vec<Sample> {
        self.all()
            .into_iter()
            .filter(|s| s.subsystem == "shop")
            .collect()
    }

    async fn wait_for(&self, what: &str, mut condition: impl FnMut(&[Sample]) -> bool) {
        let started = std::time::Instant::now();
        while !condition(&self.all()) {
            assert!(started.elapsed() < PATIENCE, "timed out waiting for {what}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Returns once every request sent so far has been decided: a request to the
    /// sentinel is always sampled, and it is sent after the others finished.
    async fn settle(&self) {
        let before = self
            .all()
            .iter()
            .filter(|s| s.subsystem == "sentinel")
            .count();
        assert_eq!(get(self.addr, SENTINEL_HOST, "/", "").await.status, 200);
        self.wait_for("the sentinel sample", |samples| {
            samples.iter().filter(|s| s.subsystem == "sentinel").count() > before
        })
        .await;
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        if let Some(plane) = self.plane.take() {
            plane.stopped(Duration::from_secs(2)).await;
        }
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown();
        }
    }
}

fn respond(status: u16, body: &'static str) -> Behavior {
    Behavior::Respond {
        status,
        headers: Vec::new(),
        body,
    }
}

fn shop_with(
    upstreams: &[&Arc<TestUpstream>],
    routes: Vec<fast_proxy_model::RouteConfig>,
) -> fast_proxy_model::SubsystemConfig {
    let addrs: Vec<_> = upstreams.iter().map(|u| u.addr).collect();
    site_to_group(
        1,
        "shop",
        HOST,
        &addrs,
        fast_proxy_model::GroupPolicy::default(),
        routes,
    )
}

fn json(sample: &Sample) -> String {
    sample.to_detail_json()
}

async fn post_json(addr: std::net::SocketAddr, path: &str, body: &str) -> Reply {
    send_raw(
        addr,
        &format!(
            "POST {path} HTTP/1.1\r\nHost: {HOST}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    )
    .await
}

// ---- which requests are sampled ----

#[tokio::test]
async fn an_ordinary_request_and_an_upstreams_own_errors_are_not_sampled() {
    let ok = TestUpstream::start(respond(200, "fine")).await;
    let broken = TestUpstream::start(respond(500, "oops")).await;
    let gateway = TestUpstream::start(respond(502, "bad gateway")).await;
    let proxy = start(
        shop_with(&[&ok], vec![route(1, "/ok")]),
        Setup::new(quiet()),
    )
    .await;
    // Three routes would need three groups; use one subsystem per backend instead.
    drop(proxy);
    for (backend, expected) in [(&ok, 200), (&broken, 500), (&gateway, 502)] {
        let proxy = start(
            shop_with(&[backend], vec![route(1, "/")]),
            Setup::new(quiet()),
        )
        .await;
        for _ in 0..3 {
            assert_eq!(get(proxy.addr, HOST, "/", "").await.status, expected);
        }
        proxy.settle().await;
        assert!(
            proxy.shop().is_empty(),
            "an upstream {expected} is the application's answer"
        );
        proxy.stop().await;
    }
}

#[tokio::test]
async fn a_dead_backend_is_sampled_as_an_upstream_failure_with_its_attempt() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    backend.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let proxy = start(
        shop_with(&[&backend], vec![route(1, "/")]),
        Setup::new(quiet()),
    )
    .await;
    let reply = get(
        proxy.addr,
        HOST,
        "/orders?page=2",
        "User-Agent: probe/1\r\n",
    )
    .await;
    assert_eq!(reply.status, 502);
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["upstream_failure"]);
    assert_eq!(sample.response.status, 502);
    assert_eq!(sample.request.method, "GET");
    assert_eq!(sample.request.path, "/orders");
    assert_eq!(sample.request.query.as_deref(), Some("page=2"));
    assert!(sample
        .request
        .headers
        .contains(&("user-agent".into(), "probe/1".into())));
    assert_eq!(sample.route, "/");
    assert_eq!(sample.instance, "test");
    assert!(!sample.request_id.is_empty());
    assert_eq!(
        sample.upstream.attempts[0].endpoint,
        backend.addr.to_string()
    );
    assert_eq!(
        sample.upstream.attempts[0].failure.as_deref(),
        Some("connect_refused")
    );
    assert_eq!(proxy.metrics.sample_totals().0, 1);
    proxy.stop().await;
}

#[tokio::test]
async fn a_connectivity_failure_that_was_retried_to_an_answer_is_sampled_as_retried() {
    let live = TestUpstream::start(respond(200, "ok")).await;
    let dead = TestUpstream::start(respond(200, "ok")).await;
    dead.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let proxy = start(
        shop_with(&[&dead, &live], vec![route(1, "/")]),
        Setup::new(quiet()),
    )
    .await;
    for _ in 0..12 {
        assert_eq!(
            get(proxy.addr, HOST, "/", "").await.status,
            200,
            "the retry hides the failure"
        );
    }
    proxy.settle().await;
    let samples = proxy.shop();
    assert!(
        !samples.is_empty(),
        "some request tried the dead backend first"
    );
    for sample in &samples {
        assert_eq!(sample.triggers, ["retried"], "never an upstream failure");
        assert_eq!(sample.response.status, 200);
        assert_eq!(sample.upstream.attempts.len(), 2);
        assert_eq!(
            sample.upstream.attempts[0].failure.as_deref(),
            Some("connect_refused")
        );
        assert_eq!(sample.upstream.attempts[1].status, Some(200));
        assert_eq!(sample.upstream.attempts[1].endpoint, live.addr.to_string());
    }
    proxy.stop().await;
}

#[tokio::test]
async fn a_retry_after_an_upstream_503_is_not_sampled() {
    let flaky = TestUpstream::start(respond(200, "ok")).await;
    let steady = TestUpstream::start(respond(200, "ok")).await;
    flaky.fail_with(503);
    let proxy = start(
        shop_with(&[&flaky, &steady], vec![route(1, "/")]),
        Setup::new(quiet()),
    )
    .await;
    for _ in 0..10 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    }
    assert!(flaky.request_count() > 0, "the failing backend was tried");
    proxy.settle().await;
    assert!(
        proxy.shop().is_empty(),
        "a 503 from the upstream is not connectivity"
    );
    proxy.stop().await;
}

#[tokio::test]
async fn a_response_timeout_is_an_upstream_failure_named_response_timeout() {
    let hung = TestUpstream::start(Behavior::Sleep(Duration::from_secs(5))).await;
    let mut slow_route = route(1, "/");
    slow_route.response_timeout = Duration::from_millis(200);
    slow_route.retry.max_attempts = 1;
    let proxy = start(shop_with(&[&hung], vec![slow_route]), Setup::new(quiet())).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 504);
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers[0], "upstream_failure");
    assert_eq!(sample.response.status, 504);
    assert_eq!(
        sample.upstream.attempts[0].failure.as_deref(),
        Some("response_timeout")
    );
    assert!(sample.total_ms >= 200);
    proxy.stop().await;
}

#[tokio::test]
async fn a_slow_request_is_sampled_and_a_fast_one_is_not() {
    let slow = TestUpstream::start(Behavior::Sleep(Duration::from_millis(300))).await;
    let mut settings = quiet();
    settings.slow_threshold_ms = 150;
    let proxy = start(
        shop_with(&[&slow], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["slow"]);
    assert!(sample.total_ms >= 150);
    proxy.stop().await;

    let fast = TestUpstream::start(respond(200, "ok")).await;
    let mut settings = quiet();
    settings.slow_threshold_ms = 150;
    let proxy = start(
        shop_with(&[&fast], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 200);
    proxy.settle().await;
    assert!(proxy.shop().is_empty());
    proxy.stop().await;
}

#[tokio::test]
async fn the_baseline_samples_ordinary_requests_by_its_rate() {
    let ok = TestUpstream::start(respond(200, "ok")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let proxy = start(shop_with(&[&ok], vec![route(1, "/")]), Setup::new(settings)).await;
    for _ in 0..5 {
        get(proxy.addr, HOST, "/", "").await;
    }
    proxy
        .wait_for("five baseline samples", |s| {
            s.iter().filter(|x| x.subsystem == "shop").count() == 5
        })
        .await;
    assert!(proxy.shop().iter().all(|s| s.triggers == ["baseline"]));
    proxy.stop().await;
}

#[tokio::test]
async fn oversized_requests_and_responses_are_sampled() {
    let echo = TestUpstream::start(Behavior::CountBody).await;
    let mut settings = quiet();
    settings.oversized_request_bytes = 10;
    let proxy = start(
        shop_with(&[&echo], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    post_json(proxy.addr, "/", "{\"twenty\":\"bytes!!!\"}").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["oversized_request"]);
    assert_eq!(
        sample.request.bytes, 21,
        "counted as the body streamed, with bodies off"
    );
    proxy.stop().await;

    let big = TestUpstream::start(respond(200, "0123456789abcdef0123456789abcdef")).await;
    let mut settings = quiet();
    settings.oversized_response_bytes = 20;
    let proxy = start(
        shop_with(&[&big], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    get(proxy.addr, HOST, "/", "").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["oversized_response"]);
    assert_eq!(sample.response.bytes, 32);
    proxy.stop().await;
}

#[tokio::test]
async fn unusual_headers_are_sampled_and_say_why() {
    let ok = TestUpstream::start(respond(200, "ok")).await;
    let proxy = start(shop_with(&[&ok], vec![route(1, "/")]), Setup::new(quiet())).await;
    let huge = format!("X-Big: {}\r\n", "a".repeat(9_000));
    get(proxy.addr, HOST, "/", &huge).await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["unusual_headers"]);
    assert_eq!(sample.request.unusual, ["huge_header_value"]);
    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_keeps_getting_4xx_is_sampled_when_it_crosses_the_threshold() {
    let missing = TestUpstream::start(respond(404, "nope")).await;
    let mut settings = quiet();
    settings.client_errors_per_min = 3;
    let proxy = start(
        shop_with(&[&missing], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    for _ in 0..2 {
        get(proxy.addr, HOST, "/", "").await;
    }
    proxy.settle().await;
    assert!(proxy.shop().is_empty(), "under the threshold");
    get(proxy.addr, HOST, "/", "").await;
    proxy
        .wait_for("the crossing", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    assert_eq!(proxy.shop()[0].triggers, ["client_errors"]);
    proxy.stop().await;
}

#[tokio::test]
async fn an_unknown_host_and_a_maintenance_answer_are_not_sampled() {
    let ok = TestUpstream::start(respond(200, "ok")).await;
    let mut setup = Setup::new(quiet());
    setup.maintenance = MaintenanceInputs {
        scopes: vec![(
            MaintenanceTarget::Subsystem(SubsystemId(1)),
            MaintenanceSettings {
                message: "back soon".into(),
                body: None,
                retry_after_secs: None,
                bypass: fast_proxy_model::BypassRules::default(),
                expires_at_ms: None,
                set_by: "ops".into(),
                set_at_ms: 0,
            },
        )],
        now_ms: 0,
    };
    let proxy = start(shop_with(&[&ok], vec![route(1, "/")]), setup).await;
    assert_eq!(
        get(proxy.addr, "unknown.example.com", "/", "").await.status,
        404
    );
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 503);
    proxy.settle().await;
    assert!(proxy.all().iter().all(|s| s.subsystem == "sentinel"));
    proxy.stop().await;
}

#[tokio::test]
async fn a_route_with_sampling_off_is_never_sampled() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    backend.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut setup = Setup::new(quiet());
    let mut off = an_override();
    off.enabled = Some(false);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), off));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 502);
    proxy.settle().await;
    assert!(proxy.shop().is_empty());
    proxy.stop().await;
}

#[tokio::test]
async fn a_client_that_leaves_while_the_upstream_is_slow_is_sampled_as_slow() {
    let slow = TestUpstream::start(Behavior::HoldUntilDropped).await;
    let mut settings = quiet();
    settings.slow_threshold_ms = 100;
    let proxy = start(
        shop_with(&[&slow], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    let mut stream = tokio::net::TcpStream::connect(proxy.addr).await.unwrap();
    tokio::io::AsyncWriteExt::write_all(
        &mut stream,
        format!("GET / HTTP/1.1\r\nHost: {HOST}\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(stream);
    proxy
        .wait_for("the cancelled request", |s| {
            s.iter().any(|x| x.subsystem == "shop")
        })
        .await;
    let sample = proxy.shop().remove(0);
    assert_eq!(sample.triggers, ["slow"]);
    assert_eq!(
        sample.response.status, 499,
        "recorded as the client closing the request"
    );
    proxy.stop().await;
}

// ---- redaction ----

#[tokio::test]
async fn no_secret_reaches_the_sample() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    backend.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let proxy = start(
        shop_with(&[&backend], vec![route(1, "/")]),
        Setup::new(quiet()),
    )
    .await;
    get(
        proxy.addr,
        HOST,
        "/login?token=ZQ-query-secret&user=ann&api_key=ZQ2",
        "Authorization: Bearer ZQ-bearer\r\nCookie: session=ZQ-cookie\r\nX-Api-Key: ZQ-key\r\n",
    )
    .await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    let text = json(&sample);
    assert!(!text.contains("ZQ"), "{text}");
    assert_eq!(
        sample.request.query.as_deref(),
        Some("token=[redacted]&user=ann&api_key=[redacted]")
    );
    proxy.stop().await;
}

// ---- cost limits ----

#[tokio::test]
async fn an_outage_keeps_a_burst_of_samples_counts_the_rest_and_reports_them_later() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    backend.kill();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let other = TestUpstream::start(Behavior::Sleep(Duration::from_millis(250))).await;
    let mut settings = quiet();
    settings.storm_burst = 3;
    settings.storm_interval_secs = 1;
    settings.slow_threshold_ms = 100;
    let down = shop_with(&[&backend], vec![route(1, "/")]);
    let mut shop = down;
    // A second group would need a second subsystem; the slow route shares the dead group's subsystem name
    // through its own upstream below.
    shop.upstream_groups
        .push(fast_proxy_model::UpstreamGroupConfig {
            id: fast_proxy_model::UpstreamGroupId(2),
            name: "slow".into(),
            policy: fast_proxy_model::GroupPolicy::default(),
            endpoints: vec![fast_proxy_model::EndpointConfig {
                id: fast_proxy_model::EndpointId(10),
                addr: fast_proxy_model::EndpointAddr::parse(&other.addr.to_string()).unwrap(),
                weight: 1,
                state: fast_proxy_model::EndpointState::Enabled,
            }],
        });
    let mut slow_route = route(2, "/slow");
    slow_route.upstream_group_id = fast_proxy_model::UpstreamGroupId(2);
    shop.routes.push(slow_route);
    let proxy = start(shop, Setup::new(settings)).await;

    for _ in 0..30 {
        assert_eq!(get(proxy.addr, HOST, "/", "").await.status, 502);
    }
    proxy.settle().await;
    let kept = proxy.shop().len();
    assert_eq!(kept, 3, "the burst, no more");
    let rendered = proxy.metrics.render();
    assert!(
        rendered.contains("fastproxy_samples_dropped_total{reason=\"storm_guard\"} 27"),
        "{rendered}"
    );

    // Another route's slow request is not starved by the outage.
    assert_eq!(get(proxy.addr, HOST, "/slow", "").await.status, 200);
    proxy
        .wait_for("the slow sample", |s| {
            s.iter().any(|x| x.triggers == ["slow"])
        })
        .await;

    // After the interval the next failure is kept and says how many were lost.
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    get(proxy.addr, HOST, "/", "").await;
    proxy
        .wait_for("the sample that reports the outage", |s| {
            s.iter()
                .any(|x| x.suppressed.as_ref().is_some_and(|n| n.count >= 27))
        })
        .await;
    proxy.stop().await;
}

#[tokio::test]
async fn the_rate_limit_caps_how_many_samples_are_kept() {
    let ok = TestUpstream::start(respond(200, "ok")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    settings.rate_per_sec = 1;
    settings.rate_burst = 2;
    let proxy = start(shop_with(&[&ok], vec![route(1, "/")]), Setup::new(settings)).await;
    for _ in 0..12 {
        get(proxy.addr, HOST, "/", "").await;
    }
    // The sentinel is limited by the same bucket, so wait on the counter, not on it.
    let started = std::time::Instant::now();
    loop {
        let rendered = proxy.metrics.render();
        if rendered.contains("fastproxy_samples_dropped_total{reason=\"rate_limit\"}") {
            break;
        }
        assert!(
            started.elapsed() < PATIENCE,
            "no request was limited: {rendered}"
        );
        get(proxy.addr, HOST, "/", "").await;
    }
    assert!(
        proxy.shop().len() <= 4,
        "the burst of two plus what refilled: {}",
        proxy.shop().len()
    );
    proxy.stop().await;
}

// ---- bodies ----

#[tokio::test]
async fn bodies_are_not_captured_unless_a_route_opts_in() {
    let backend = TestUpstream::start(respond(200, "{\"ok\":true}")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let proxy = start(
        shop_with(&[&backend], vec![route(1, "/")]),
        Setup::new(settings),
    )
    .await;
    post_json(proxy.addr, "/", "{\"password\":\"ZQ\"}").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert!(sample.request.body.is_none() && sample.response.body.is_none());
    assert_eq!(sample.response.body_omitted.as_deref(), Some("off"));
    assert!(!json(&sample).contains("ZQ"));
    proxy.stop().await;
}

#[tokio::test]
async fn an_opted_in_route_captures_redacted_bodies_both_ways() {
    let backend = TestUpstream::start(Behavior::Respond {
        status: 200,
        headers: vec![("content-type", "application/json")],
        body: "{\"token\":\"ZQ-response\",\"ok\":true}",
    })
    .await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let mut setup = Setup::new(settings);
    let mut bodies = an_override();
    bodies.capture_bodies = Some(true);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), bodies));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    post_json(proxy.addr, "/", "{\"password\":\"ZQ-request\",\"x\":1}").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    let request = sample.request.body.as_ref().unwrap();
    assert_eq!(request.text, "{\"password\":\"[redacted]\",\"x\":1}");
    assert!(!request.truncated);
    let response = sample.response.body.as_ref().unwrap();
    assert_eq!(response.text, "{\"token\":\"[redacted]\",\"ok\":true}");
    assert!(!json(&sample).contains("ZQ"));
    proxy.stop().await;
}

#[tokio::test]
async fn a_body_longer_than_its_cap_is_cut_and_says_so() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    settings.body_request_cap = 10;
    let mut setup = Setup::new(settings);
    let mut bodies = an_override();
    bodies.capture_bodies = Some(true);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), bodies));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    post_json(proxy.addr, "/", "{\"note\":\"a long free text body\"}").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let body = proxy.shop().remove(0).request.body.unwrap();
    assert_eq!(body.text, "{\"note\":\"a");
    assert!(body.truncated);
    proxy.stop().await;
}

#[tokio::test]
async fn a_spent_body_budget_skips_the_bodies_but_not_the_sample() {
    let backend = TestUpstream::start(respond(200, "ok")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let mut setup = Setup::new(settings);
    setup.body_budget_bytes = 100;
    let mut bodies = an_override();
    bodies.capture_bodies = Some(true);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), bodies));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    post_json(proxy.addr, "/", "{\"a\":1}").await;
    proxy
        .wait_for("the sample", |s| s.iter().any(|x| x.subsystem == "shop"))
        .await;
    let sample = proxy.shop().remove(0);
    assert!(sample.request.body.is_none());
    assert_eq!(sample.response.body_omitted.as_deref(), Some("budget"));
    assert!(proxy
        .metrics
        .render()
        .contains("fastproxy_samples_dropped_total{reason=\"budget\"} 1"));
    proxy.stop().await;
}

#[tokio::test]
async fn the_body_budget_is_returned_after_every_request() {
    let backend = TestUpstream::start(respond(200, "{\"ok\":true}")).await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let mut setup = Setup::new(settings);
    let mut bodies = an_override();
    bodies.capture_bodies = Some(true);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), bodies));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    let budget = proxy.runtime.as_ref().unwrap().handle().budget().clone();
    for _ in 0..5 {
        post_json(proxy.addr, "/", "{\"a\":1}").await;
    }
    proxy.settle().await;
    proxy
        .wait_for("every shop sample", |s| {
            s.iter().filter(|x| x.subsystem == "shop").count() >= 1
        })
        .await;
    let started = std::time::Instant::now();
    while budget.used() != 0 {
        assert!(
            started.elapsed() < PATIENCE,
            "{} bytes are still reserved",
            budget.used()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    proxy.stop().await;
}

// ---- bookkeeping ----

#[tokio::test]
async fn the_routes_rolling_latency_is_fed_by_the_requests_it_serves() {
    // Loopback answers in under a millisecond, which a histogram of milliseconds
    // cannot tell from nothing, so the backend takes a few.
    let ok = TestUpstream::start(Behavior::Sleep(Duration::from_millis(5))).await;
    let proxy = start(shop_with(&[&ok], vec![route(1, "/")]), Setup::new(quiet())).await;
    for _ in 0..(fast_proxy_sampling::MIN_REQUESTS + 5) {
        get(proxy.addr, HOST, "/", "").await;
    }
    proxy.settle().await;
    let snapshot = proxy.holder.load();
    let window = snapshot.sampling.window("shop", "/").unwrap();
    window.publish(fast_proxy_balancing::monotonic_ms());
    assert!(
        window.p99_ms() > 0,
        "a p99 is published once enough requests were seen"
    );
    proxy.stop().await;
}

#[tokio::test]
async fn sampling_does_not_change_what_the_client_sees() {
    let backend = TestUpstream::start(Behavior::Respond {
        status: 201,
        headers: vec![("x-custom", "kept")],
        body: "created",
    })
    .await;
    let mut settings = quiet();
    settings.baseline_per_mille = 1000;
    let mut setup = Setup::new(settings);
    let mut bodies = an_override();
    bodies.capture_bodies = Some(true);
    setup
        .overrides
        .push((SamplingTarget::Subsystem(SubsystemId(1)), bodies));
    let proxy = start(shop_with(&[&backend], vec![route(1, "/")]), setup).await;
    let reply = post_json(proxy.addr, "/things", "{\"a\":1}").await;
    assert_eq!(reply.status, 201);
    assert_eq!(reply.header("x-custom"), Some("kept"));
    assert_eq!(reply.body, "created");
    assert_eq!(backend.last_path(), "/things");
    let seen = backend
        .requests
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .body
        .clone();
    assert_eq!(seen, b"{\"a\":1}", "the upstream got the whole body");
    let _ = &proxy.sentinel;
    proxy.stop().await;
}
