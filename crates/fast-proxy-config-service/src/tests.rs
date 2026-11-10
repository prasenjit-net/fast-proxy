use std::sync::Arc;
use std::time::Duration;

use fast_proxy_model::{
    EndpointAddr, EndpointState, GroupPolicy, HostPattern, RetryPolicy, RoutePrefix, SubsystemId,
    SubsystemName, UpstreamGroupId, UpstreamPrefix,
};
use fast_proxy_routing::Lookup;
use fast_proxy_store::repository::{hosts, routes, subsystems, upstreams};
use fast_proxy_store::{
    Connection, EndpointFields, Mutation, RouteFields, Store, StoreError, SubsystemFields,
    UpstreamFields,
};

use crate::{ConfigChanged, ConfigService, StartError};

async fn service() -> ConfigService {
    ConfigService::start(Store::open_in_memory().unwrap())
        .await
        .unwrap()
}

/// One transaction creating a subsystem with a host, an upstream and a `/` route.
pub(crate) fn create_site(
    name: &'static str,
    host: String,
    backend: &'static str,
) -> impl FnOnce(&Connection) -> Result<Mutation<SubsystemId>, StoreError> + Send + 'static {
    move |conn| {
        let id = subsystems::create(
            conn,
            &SubsystemFields {
                name: SubsystemName::parse(name).unwrap(),
                enabled: true,
                description: String::new(),
                https: fast_proxy_model::HttpsPolicy::default(),
            },
        )?;
        hosts::add(conn, id, &HostPattern::parse(&host).unwrap())?;
        let group = upstreams::create(
            conn,
            id,
            &UpstreamFields {
                name: "backend".into(),
                policy: GroupPolicy::default(),
            },
            &[EndpointFields {
                addr: EndpointAddr::parse(backend).unwrap(),
                weight: 1,
                state: EndpointState::Enabled,
            }],
        )?;
        routes::create(conn, id, &root_route(group))?;
        Ok(Mutation {
            value: id,
            summary: format!("subsystem created: {name}"),
        })
    }
}

fn root_route(group: UpstreamGroupId) -> RouteFields {
    RouteFields {
        prefix: RoutePrefix::parse("/").unwrap(),
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

fn backend_for(service: &ConfigService, host: &str) -> Option<String> {
    match service.snapshot_holder().load().lookup(host, "/") {
        Lookup::Matched { route, .. } => Some(route.route.upstream.endpoints[0].addr.to_string()),
        _ => None,
    }
}

async fn stored_state(service: &ConfigService) -> (usize, u64) {
    let read = service
        .read(|conn| Ok(subsystems::list(conn)?.len()))
        .await
        .unwrap();
    (read.value, read.version)
}

#[tokio::test]
async fn an_empty_database_starts_with_an_empty_snapshot_at_version_zero() {
    let service = service().await;
    let snapshot = service.snapshot_holder().load();
    assert_eq!(snapshot.version, 0);
    assert!(snapshot.subsystems.is_empty());
}

#[tokio::test]
async fn an_applied_change_is_live_immediately() {
    let service = service().await;
    let applied = service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    assert_eq!(applied.version, 1);
    assert_eq!(service.snapshot_holder().load().version, 1);
    assert_eq!(
        backend_for(&service, "shop.example.com").as_deref(),
        Some("127.0.0.1:3000")
    );
}

#[tokio::test]
async fn two_subsystems_route_to_different_backends() {
    let service = service().await;
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    service
        .apply(create_site(
            "blog",
            "blog.example.com".into(),
            "127.0.0.1:3001",
        ))
        .await
        .unwrap();
    assert_eq!(
        backend_for(&service, "shop.example.com").as_deref(),
        Some("127.0.0.1:3000")
    );
    assert_eq!(
        backend_for(&service, "blog.example.com").as_deref(),
        Some("127.0.0.1:3001")
    );
    assert_eq!(service.snapshot_holder().load().version, 2);
}

#[tokio::test]
async fn a_change_that_fails_to_compile_leaves_the_database_and_snapshot_untouched() {
    let service = service().await;
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    let before = stored_state(&service).await;

    // The store's own API cannot do this; raw SQL removes the group's only
    // endpoint, which the compiler rejects.
    let err = service
        .apply(|conn| {
            conn.execute("DELETE FROM upstream_endpoint", [])?;
            Ok(Mutation {
                value: (),
                summary: "sneaky".into(),
            })
        })
        .await
        .unwrap_err();

    assert!(
        matches!(&err, StoreError::Invalid(f) if f[0].message.contains("1 to 64 endpoints")),
        "{err}"
    );
    assert_eq!(stored_state(&service).await, before);
    assert_eq!(service.snapshot_holder().load().version, 1);
    assert_eq!(
        backend_for(&service, "shop.example.com").as_deref(),
        Some("127.0.0.1:3000")
    );
}

#[tokio::test]
async fn a_store_error_leaves_the_snapshot_untouched() {
    let service = service().await;
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    let err = service
        .apply(create_site(
            "shop",
            "other.example.com".into(),
            "127.0.0.1:3001",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::Conflict(_)));
    assert_eq!(service.snapshot_holder().load().version, 1);
    assert_eq!(backend_for(&service, "other.example.com"), None);
}

#[tokio::test]
async fn rollback_makes_the_earlier_configuration_live_as_a_new_revision() {
    let service = service().await;
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    let shop = SubsystemId(1);
    service
        .apply(move |conn| {
            subsystems::delete(conn, shop)?;
            Ok(Mutation {
                value: (),
                summary: "subsystem deleted".into(),
            })
        })
        .await
        .unwrap();
    assert_eq!(backend_for(&service, "shop.example.com"), None);

    let applied = service.rollback(1).await.unwrap();
    assert_eq!(applied.version, 3);
    assert_eq!(service.snapshot_holder().load().version, 3);
    assert_eq!(
        backend_for(&service, "shop.example.com").as_deref(),
        Some("127.0.0.1:3000")
    );
}

#[tokio::test]
async fn rolling_back_to_a_missing_revision_is_not_found_and_changes_nothing() {
    let service = service().await;
    let err = service.rollback(42).await.unwrap_err();
    assert!(matches!(err, StoreError::NotFound(_)));
    assert_eq!(service.snapshot_holder().load().version, 0);
}

#[tokio::test]
async fn every_applied_change_is_broadcast_with_its_summary() {
    let service = service().await;
    let mut events = service.subscribe();
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    service.rollback(1).await.unwrap();

    assert_eq!(
        events.recv().await.unwrap(),
        ConfigChanged {
            version: 1,
            summary: "subsystem created: shop".into()
        }
    );
    assert_eq!(
        events.recv().await.unwrap(),
        ConfigChanged {
            version: 2,
            summary: "rollback to v1".into()
        }
    );
}

#[tokio::test]
async fn failed_changes_are_not_broadcast() {
    let service = service().await;
    let mut events = service.subscribe();
    service.rollback(9).await.unwrap_err();
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn the_configuration_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    {
        let service = ConfigService::start(Store::open(&path).unwrap())
            .await
            .unwrap();
        service
            .apply(create_site(
                "shop",
                "shop.example.com".into(),
                "127.0.0.1:3000",
            ))
            .await
            .unwrap();
    }
    let service = ConfigService::start(Store::open(&path).unwrap())
        .await
        .unwrap();
    assert_eq!(service.snapshot_holder().load().version, 1);
    assert_eq!(
        backend_for(&service, "shop.example.com").as_deref(),
        Some("127.0.0.1:3000")
    );
}

#[tokio::test]
async fn starting_on_a_stored_configuration_that_does_not_compile_fails() {
    let store = Store::open_in_memory().unwrap();
    store
        .run(|db| {
            db.write(
                |conn| {
                    conn.execute(
                        "INSERT INTO subsystem (id, name, created_at, updated_at) VALUES (1, 'a', 0, 0)",
                        [],
                    )?;
                    conn.execute(
                        "INSERT INTO upstream_group (id, subsystem_id, name) VALUES (1, 1, 'g')",
                        [],
                    )?;
                    // A group without an endpoint does not compile.
                    conn.execute(
                        "INSERT INTO route (subsystem_id, prefix, upstream_group_id)
                         VALUES (1, '/', 1)",
                        [],
                    )?;
                    Ok(Mutation {
                        value: (),
                        summary: "bad".into(),
                    })
                },
                |_| Ok(()),
            )
        })
        .await
        .unwrap();
    let err = ConfigService::start(store).await.err().unwrap();
    assert!(matches!(err, StartError::Compile(_)), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_applies_are_serialized_and_the_snapshot_matches_the_database() {
    let service = Arc::new(service().await);
    let tasks: Vec<_> = (0..8)
        .map(|n| {
            let service = service.clone();
            tokio::spawn(async move {
                let name: &'static str = Box::leak(format!("site{n}").into_boxed_str());
                service
                    .apply(create_site(
                        name,
                        format!("{name}.example.com"),
                        "127.0.0.1:3000",
                    ))
                    .await
                    .unwrap()
                    .version
            })
        })
        .collect();
    let mut versions = Vec::new();
    for task in tasks {
        versions.push(task.await.unwrap());
    }
    versions.sort_unstable();
    assert_eq!(versions, (1..=8).collect::<Vec<u64>>());

    let snapshot = service.snapshot_holder().load();
    assert_eq!(snapshot.version, 8);
    assert_eq!(snapshot.subsystems.len(), 8);
    assert_eq!(stored_state(&service).await, (8, 8));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_concurrent_reader_never_observes_a_partial_state() {
    let service = Arc::new(service().await);
    let holder = service.snapshot_holder();
    let reader = tokio::spawn(async move {
        let mut last_version = 0;
        let mut seen = 0;
        while last_version < 20 {
            let snapshot = holder.load();
            assert!(snapshot.version >= last_version, "version went backwards");
            last_version = snapshot.version;
            // Every change adds a whole site in one transaction, so a host
            // that resolves must always have its route.
            for n in 0..20 {
                let host = format!("site{n}.example.com");
                match snapshot.lookup(&host, "/") {
                    Lookup::UnknownHost => {}
                    Lookup::Matched { .. } => seen += 1,
                    other => panic!("partial state for {host}: {other:?}"),
                }
            }
            tokio::task::yield_now().await;
        }
        seen
    });
    for n in 0..20 {
        let name: &'static str = Box::leak(format!("site{n}").into_boxed_str());
        service
            .apply(create_site(
                name,
                format!("{name}.example.com"),
                "127.0.0.1:3000",
            ))
            .await
            .unwrap();
    }
    assert!(reader.await.unwrap() > 0);
}

// ---- metrics ------------------------------------------------------------

fn metric(metrics: &fast_proxy_telemetry::Metrics, prefix: &str) -> Option<f64> {
    metrics
        .render()
        .lines()
        .find(|line| line.starts_with(prefix))
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
}

#[tokio::test]
async fn the_config_version_and_reload_outcomes_are_reported_to_metrics() {
    let metrics = Arc::new(fast_proxy_telemetry::Metrics::new());
    let service =
        ConfigService::start_with_metrics(Store::open_in_memory().unwrap(), metrics.clone())
            .await
            .unwrap();
    assert_eq!(metric(&metrics, "fastproxy_config_version"), Some(0.0));

    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    assert_eq!(metric(&metrics, "fastproxy_config_version"), Some(1.0));

    // A conflicting change and a failed rollback are both rejected.
    service
        .apply(create_site(
            "shop",
            "other.example.com".into(),
            "127.0.0.1:3001",
        ))
        .await
        .unwrap_err();
    service.rollback(99).await.unwrap_err();
    service.rollback(1).await.unwrap();

    let applied = r#"fastproxy_config_reloads_total{outcome="applied"}"#;
    let rejected = r#"fastproxy_config_reloads_total{outcome="rejected"}"#;
    assert_eq!(metric(&metrics, applied), Some(2.0));
    assert_eq!(metric(&metrics, rejected), Some(2.0));
    assert_eq!(metric(&metrics, "fastproxy_config_version"), Some(2.0));
}

#[tokio::test]
async fn starting_on_stored_config_reports_its_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fast-proxy.db");
    {
        let service = ConfigService::start(Store::open(&path).unwrap())
            .await
            .unwrap();
        service
            .apply(create_site(
                "shop",
                "shop.example.com".into(),
                "127.0.0.1:3000",
            ))
            .await
            .unwrap();
    }
    let metrics = Arc::new(fast_proxy_telemetry::Metrics::new());
    ConfigService::start_with_metrics(Store::open(&path).unwrap(), metrics.clone())
        .await
        .unwrap();
    assert_eq!(metric(&metrics, "fastproxy_config_version"), Some(1.0));
}

fn live_runtime(service: &ConfigService, host: &str) -> Arc<fast_proxy_balancing::EndpointRuntime> {
    let snapshot = service.snapshot_holder().load();
    let Lookup::Matched { route, .. } = snapshot.lookup(host, "/") else {
        panic!("no route for {host}");
    };
    route.route.upstream.candidates[0].runtime.clone()
}

#[tokio::test]
async fn endpoint_state_survives_unrelated_changes_and_resets_when_the_address_changes() {
    let service = service().await;
    let site = service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap()
        .value;
    let runtime = live_runtime(&service, "shop.example.com");
    runtime.record_probe(false, 1, 1);
    assert!(!runtime.is_active_healthy());

    service
        .apply(move |conn| {
            let mut fields = subsystem_fields_for("shop");
            fields.description = "unrelated edit".into();
            subsystems::update(conn, site, &fields)?;
            Ok(Mutation {
                value: (),
                summary: "edit".into(),
            })
        })
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(&runtime, &live_runtime(&service, "shop.example.com")),
        "an unrelated edit keeps the runtime"
    );
    assert!(!live_runtime(&service, "shop.example.com").is_active_healthy());

    service
        .apply(move |conn| {
            let group = subsystems::get(conn, site)?.upstream_groups[0].clone();
            let moved = EndpointFields {
                addr: EndpointAddr::parse("127.0.0.1:3999").unwrap(),
                weight: 1,
                state: EndpointState::Enabled,
            };
            fast_proxy_store::repository::endpoints::update(
                conn,
                site,
                group.id,
                group.endpoints[0].id,
                &moved,
            )?;
            Ok(Mutation {
                value: (),
                summary: "move".into(),
            })
        })
        .await
        .unwrap();
    let fresh = live_runtime(&service, "shop.example.com");
    assert!(!Arc::ptr_eq(&runtime, &fresh));
    assert!(fresh.is_active_healthy(), "a moved endpoint starts healthy");
}

#[tokio::test]
async fn a_rejected_change_keeps_the_live_runtime_state() {
    let service = service().await;
    service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap();
    let runtime = live_runtime(&service, "shop.example.com");
    runtime.record_probe(false, 1, 1);
    let rejected = service
        .apply(|conn| {
            conn.execute("DELETE FROM upstream_endpoint", [])?;
            Ok(Mutation {
                value: (),
                summary: "bad".into(),
            })
        })
        .await;
    assert!(rejected.is_err());
    let after = live_runtime(&service, "shop.example.com");
    assert!(Arc::ptr_eq(&runtime, &after) && !after.is_active_healthy());
}

fn subsystem_fields_for(name: &str) -> SubsystemFields {
    SubsystemFields {
        name: SubsystemName::parse(name).unwrap(),
        enabled: true,
        description: String::new(),
        https: fast_proxy_model::HttpsPolicy::default(),
    }
}

#[tokio::test]
async fn removing_an_endpoint_deletes_its_metric_series() {
    use fast_proxy_telemetry::{EndpointLabels, Metrics, UpstreamOutcome};
    let metrics = Arc::new(Metrics::new());
    let service =
        ConfigService::start_with_metrics(Store::open_in_memory().unwrap(), metrics.clone())
            .await
            .unwrap();
    let site = service
        .apply(create_site(
            "shop",
            "shop.example.com".into(),
            "127.0.0.1:3000",
        ))
        .await
        .unwrap()
        .value;
    let second = service
        .apply(move |conn| {
            let group = subsystems::get(conn, site)?.upstream_groups[0].id;
            let added = EndpointFields {
                addr: EndpointAddr::parse("127.0.0.1:3001").unwrap(),
                weight: 1,
                state: EndpointState::Enabled,
            };
            let id = fast_proxy_store::repository::endpoints::add(conn, site, group, &added)?;
            Ok(Mutation {
                value: (group, id),
                summary: "add".into(),
            })
        })
        .await
        .unwrap()
        .value;
    for endpoint in ["127.0.0.1:3000", "127.0.0.1:3001"] {
        let labels = EndpointLabels {
            subsystem: "shop",
            upstream: "backend",
            endpoint,
        };
        metrics.upstream_request(labels, UpstreamOutcome::Success);
    }
    assert!(metrics.render().contains("127.0.0.1:3001"));

    service
        .apply(move |conn| {
            fast_proxy_store::repository::endpoints::delete(conn, site, second.0, second.1)?;
            Ok(Mutation {
                value: (),
                summary: "remove".into(),
            })
        })
        .await
        .unwrap();
    let text = metrics.render();
    assert!(text.contains("127.0.0.1:3000"));
    assert!(!text.contains("127.0.0.1:3001"), "{text}");
}
