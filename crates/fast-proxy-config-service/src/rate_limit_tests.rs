//! Rate limits through the config service: a policy is a configuration change,
//! and a client's budget survives unrelated changes, reloads and the capacity a
//! process starts with (ADR 0030).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_limits::Admission;
use fast_proxy_model::{RateLimitKey, RateLimitPolicy, SubsystemId, DEFAULT_IPV6_PREFIX};
use fast_proxy_store::repository::rate_limits;
use fast_proxy_store::{Mutation, Store};

use crate::tests::create_site;
use crate::{Audited, ChangeContext, ConfigService, TlsSettings};

fn ann() -> ChangeContext {
    ChangeContext::token("ann", "admin", "10.0.0.5", "req-1")
}

async fn add_site(service: &ConfigService, name: &'static str) -> SubsystemId {
    service
        .apply(
            &ann(),
            Audited::new("subsystem.create", name),
            create_site(name, format!("{name}.example.com"), "127.0.0.1:3000"),
        )
        .await
        .unwrap()
        .value
}

async fn limit(service: &ConfigService, subsystem: SubsystemId) {
    let policy = RateLimitPolicy {
        rate: 1,
        period: Duration::from_secs(60),
        burst: 1,
        key: RateLimitKey::Client,
        ipv6_prefix: DEFAULT_IPV6_PREFIX,
    };
    service
        .apply(
            &ann(),
            Audited::new("rate_limit.set", "shop"),
            move |conn| {
                rate_limits::set(conn, subsystem, None, &policy)?;
                Ok(Mutation {
                    value: (),
                    summary: "rate limit set: shop".into(),
                })
            },
        )
        .await
        .unwrap();
}

fn check(service: &ConfigService, subsystem: SubsystemId, client: &str) -> Admission {
    let client: IpAddr = client.parse().unwrap();
    let snapshot = service.snapshot_holder().load();
    snapshot.subsystems[&subsystem]
        .routes
        .find("/")
        .unwrap()
        .route
        .rate_limit
        .as_ref()
        .expect("the subsystem's policy applies to its route")
        .check(client, None, 1)
}

#[tokio::test]
async fn a_clients_budget_survives_other_changes_and_a_reload() {
    let service = ConfigService::start(Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let shop = add_site(&service, "shop").await;
    limit(&service, shop).await;
    assert_eq!(check(&service, shop, "192.0.2.1"), Admission::Allowed);

    add_site(&service, "blog").await;
    assert!(
        matches!(
            check(&service, shop, "192.0.2.1"),
            Admission::Limited { .. }
        ),
        "an unrelated change keeps the table"
    );
    service.reload().await.unwrap();
    assert!(
        matches!(
            check(&service, shop, "192.0.2.1"),
            Admission::Limited { .. }
        ),
        "and so does a reload"
    );
    assert_eq!(check(&service, shop, "192.0.2.2"), Admission::Allowed);
}

#[tokio::test]
async fn the_key_capacity_given_at_start_bounds_every_table() {
    let service = Arc::new(
        ConfigService::start_with_limits(
            Store::open_in_memory().unwrap(),
            Arc::default(),
            TlsSettings::default(),
            2,
        )
        .await
        .unwrap(),
    );
    let shop = add_site(&service, "shop").await;
    limit(&service, shop).await;
    for client in ["192.0.2.1", "192.0.2.2"] {
        assert_eq!(check(&service, shop, client), Admission::Allowed);
    }
    assert_eq!(
        check(&service, shop, "192.0.2.3"),
        Admission::TableFull,
        "a third client finds no room and is let through"
    );
}
