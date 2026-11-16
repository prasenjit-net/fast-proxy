//! ACME HTTP-01 challenges through the real proxy: answered from the snapshot
//! ahead of maintenance and a disabled subsystem, only for the right host and
//! method, and anything else routed to the upstream as before.

use std::sync::Arc;

use fast_proxy_model::{
    AcmeChallenge, AcmeToken, BypassRules, Config, MaintenanceSettings, MaintenanceTarget,
    SubsystemId,
};
use fast_proxy_routing::{compile_full, AcmeInputs, MaintenanceInputs, SamplingInputs, TlsInputs};

use crate::test_support::{get, route, send_raw, site_to, Harness};
use crate::test_upstream::{Behavior, TestUpstream};

const SHOP: &str = "shop.example.com";
const BLOG: &str = "blog.example.com";
const PATH: &str = "/.well-known/acme-challenge/tok-1";

struct Rig {
    proxy: Harness,
    upstream: Arc<TestUpstream>,
    config: Config,
}

impl Rig {
    async fn start() -> Self {
        let upstream = TestUpstream::start(Behavior::Respond {
            status: 200,
            headers: vec![],
            body: "from the upstream",
        })
        .await;
        let config = Config {
            subsystems: vec![
                site_to(1, "shop", SHOP, upstream.addr, vec![route(10, "/")]),
                site_to(2, "blog", BLOG, upstream.addr, vec![route(20, "/")]),
            ],
        };
        let proxy = Harness::proxying(config.clone(), &[]).await;
        let rig = Self {
            proxy,
            upstream,
            config,
        };
        rig.set(&rig.config.clone(), Vec::new());
        rig
    }

    /// Makes `config` live with the shop's challenge and `maintenance`.
    fn set(&self, config: &Config, maintenance: Vec<(MaintenanceTarget, MaintenanceSettings)>) {
        let challenge = AcmeChallenge::new(
            AcmeToken::parse("tok-1").unwrap(),
            SubsystemId(1),
            vec![SHOP.into()],
            "tok-1.thumbprint".into(),
            i64::MAX,
        )
        .unwrap();
        let snapshot = compile_full(
            2,
            config,
            &self.proxy.holder.load().runtime_registry(),
            &TlsInputs::off(),
            &MaintenanceInputs {
                scopes: maintenance,
                now_ms: 0,
            },
            &SamplingInputs::none(),
            &AcmeInputs {
                challenges: vec![challenge],
                now_ms: 0,
            },
        )
        .unwrap();
        self.proxy.holder.swap(snapshot);
    }
}

fn maintenance() -> MaintenanceSettings {
    MaintenanceSettings {
        message: "deploying".into(),
        body: None,
        retry_after_secs: None,
        bypass: BypassRules::default(),
        expires_at_ms: None,
        set_by: "ops".into(),
        set_at_ms: 0,
    }
}

#[tokio::test]
async fn a_live_token_is_answered_with_its_key_authorization_and_never_forwarded() {
    let rig = Rig::start().await;
    let reply = get(rig.proxy.addr, SHOP, PATH, "").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "tok-1.thumbprint");
    assert_eq!(reply.header("content-type"), Some("text/plain"));
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert!(reply.header("x-request-id").is_some());
    assert_eq!(rig.upstream.request_count(), 0);
    let text = rig.proxy.metrics.render();
    assert!(text.contains(r#"fastproxy_acme_challenge_requests_total{outcome="answered"} 1"#));
    assert!(!text.contains("tok-1"), "a token never becomes a label");
}

#[tokio::test]
async fn head_is_answered_too_and_other_methods_are_routed() {
    let rig = Rig::start().await;
    let head = send_raw(
        rig.proxy.addr,
        &format!("HEAD {PATH} HTTP/1.1\r\nHost: {SHOP}\r\nConnection: close\r\n\r\n"),
    )
    .await;
    assert_eq!(head.status, 200);
    assert_eq!(rig.upstream.request_count(), 0);
    let post = send_raw(
        rig.proxy.addr,
        &format!(
            "POST {PATH} HTTP/1.1\r\nHost: {SHOP}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    )
    .await;
    assert_eq!(post.body, "from the upstream");
    assert_eq!(rig.upstream.request_count(), 1);
}

#[tokio::test]
async fn an_unknown_token_or_another_host_reaches_the_upstream_as_before() {
    let rig = Rig::start().await;
    let unknown = get(
        rig.proxy.addr,
        SHOP,
        "/.well-known/acme-challenge/other",
        "",
    )
    .await;
    assert_eq!(unknown.body, "from the upstream");
    assert_eq!(
        rig.upstream.last_path(),
        "/.well-known/acme-challenge/other"
    );
    let other_host = get(rig.proxy.addr, BLOG, PATH, "").await;
    assert_eq!(other_host.body, "from the upstream");
    assert_eq!(rig.upstream.request_count(), 2);
    let text = rig.proxy.metrics.render();
    assert!(text.contains(r#"fastproxy_acme_challenge_requests_total{outcome="not_answered"} 2"#));
}

#[tokio::test]
async fn a_challenge_is_answered_in_every_maintenance_scope() {
    let rig = Rig::start().await;
    for target in [
        MaintenanceTarget::Global,
        MaintenanceTarget::Subsystem(SubsystemId(1)),
        MaintenanceTarget::Route(fast_proxy_model::RouteId(10)),
    ] {
        rig.set(&rig.config, vec![(target, maintenance())]);
        let reply = get(rig.proxy.addr, SHOP, PATH, "").await;
        assert_eq!(
            (reply.status, reply.body.as_str()),
            (200, "tok-1.thumbprint"),
            "{target:?}"
        );
        assert_eq!(
            get(rig.proxy.addr, SHOP, "/", "").await.status,
            503,
            "{target:?}"
        );
    }
    assert_eq!(rig.upstream.request_count(), 0);
}

#[tokio::test]
async fn a_challenge_is_answered_for_a_disabled_subsystem() {
    let rig = Rig::start().await;
    let mut disabled = rig.config.clone();
    disabled.subsystems[0].enabled = false;
    rig.set(&disabled, Vec::new());
    let reply = get(rig.proxy.addr, SHOP, PATH, "").await;
    assert_eq!(
        (reply.status, reply.body.as_str()),
        (200, "tok-1.thumbprint")
    );
    assert_eq!(get(rig.proxy.addr, SHOP, "/", "").await.status, 503);
}
