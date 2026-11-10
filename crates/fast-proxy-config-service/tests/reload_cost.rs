//! How long a configuration change takes with a large configuration (plan
//! task C2): thousands of routes and hundreds of endpoints. Timing depends on
//! the machine, so the measurement is `#[ignore]`d; run it in release mode:
//!
//! `cargo test -p fast-proxy-config-service --release --test reload_cost -- --ignored --nocapture`

use std::time::{Duration, Instant};

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_config_service::ConfigService;
use fast_proxy_model::{
    EndpointAddr, EndpointState, GroupPolicy, HostPattern, RetryPolicy, RoutePrefix, SubsystemId,
    SubsystemName, UpstreamPrefix,
};
use fast_proxy_routing::compile;
use fast_proxy_store::repository::{hosts, routes, subsystems, upstreams};
use fast_proxy_store::{
    EndpointFields, Mutation, RouteFields, Store, SubsystemFields, UpstreamFields,
};

const SUBSYSTEMS: usize = 200;
const ROUTES_PER_SUBSYSTEM: usize = 20;
const ENDPOINTS_PER_SUBSYSTEM: usize = 3;

/// Creates the whole large configuration in one change.
async fn populate(service: &ConfigService) {
    service
        .apply(|conn| {
            for s in 0..SUBSYSTEMS {
                let id = subsystems::create(
                    conn,
                    &SubsystemFields {
                        name: SubsystemName::parse(&format!("site{s}")).unwrap(),
                        enabled: true,
                        description: String::new(),
                        https: fast_proxy_model::HttpsPolicy::default(),
                    },
                )?;
                hosts::add(
                    conn,
                    id,
                    &HostPattern::parse(&format!("site{s}.example.com")).unwrap(),
                )?;
                let endpoints: Vec<_> = (0..ENDPOINTS_PER_SUBSYSTEM)
                    .map(|e| EndpointFields {
                        addr: EndpointAddr::parse(&format!(
                            "10.{}.{}.{}:80",
                            s / 250,
                            s % 250,
                            e + 1
                        ))
                        .unwrap(),
                        weight: 1,
                        state: EndpointState::Enabled,
                    })
                    .collect();
                let group = upstreams::create(
                    conn,
                    id,
                    &UpstreamFields {
                        name: "backend".into(),
                        policy: GroupPolicy::default(),
                    },
                    &endpoints,
                )?;
                for r in 0..ROUTES_PER_SUBSYSTEM {
                    routes::create(conn, id, &route(&format!("/r{r}"), group))?;
                }
            }
            Ok(Mutation {
                value: (),
                summary: "populate".into(),
            })
        })
        .await
        .unwrap();
}

fn route(prefix: &str, group: fast_proxy_model::UpstreamGroupId) -> RouteFields {
    RouteFields {
        prefix: RoutePrefix::parse(prefix).unwrap(),
        upstream_group_id: group,
        strip_prefix: false,
        upstream_prefix: UpstreamPrefix::default(),
        preserve_host: true,
        connect_timeout: Duration::from_secs(5),
        response_timeout: Duration::from_secs(30),
        retry: RetryPolicy::default(),
        enabled: true,
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

#[tokio::test]
#[ignore = "timing measurement; run in release mode"]
async fn report_reload_cost_with_a_large_configuration() {
    let store = Store::open_in_memory().unwrap();
    let service = ConfigService::start(store).await.unwrap();
    populate(&service).await;
    let live = service.snapshot_holder().load();
    let routes = SUBSYSTEMS * ROUTES_PER_SUBSYSTEM;
    let endpoints = SUBSYSTEMS * ENDPOINTS_PER_SUBSYSTEM;

    // Compile alone, carrying the runtime state over (what every change does).
    let config = service
        .read(fast_proxy_store::repository::load::load_all)
        .await
        .unwrap()
        .value;
    let registry = live.runtime_registry();
    let compiles: Vec<Duration> = (0..20)
        .map(|_| {
            let started = Instant::now();
            compile(1, &config, &registry).unwrap();
            started.elapsed()
        })
        .collect();
    let fresh = Instant::now();
    compile(1, &config, &RuntimeRegistry::new()).unwrap();
    let fresh = fresh.elapsed();

    // A whole change: mutate, compile as the veto, store a revision, swap.
    let target = SubsystemId(1);
    let applies: Vec<Duration> = {
        let mut samples = Vec::new();
        for round in 0..10 {
            let started = Instant::now();
            service
                .apply(move |conn| {
                    let mut fields = SubsystemFields {
                        name: SubsystemName::parse("site0").unwrap(),
                        enabled: true,
                        description: format!("edit {round}"),
                        https: fast_proxy_model::HttpsPolicy::default(),
                    };
                    fields.description.push('!');
                    subsystems::update(conn, target, &fields)?;
                    Ok(Mutation {
                        value: (),
                        summary: "edit".into(),
                    })
                })
                .await
                .unwrap();
            samples.push(started.elapsed());
        }
        samples
    };

    println!("configuration: {SUBSYSTEMS} subsystems, {routes} routes, {endpoints} endpoints");
    println!(
        "compile, carrying runtime state (median of 20): {:?}",
        median(compiles)
    );
    println!("compile, from scratch:                          {fresh:?}");
    println!(
        "whole change, small edit (median of 10):        {:?}",
        median(applies)
    );
}
