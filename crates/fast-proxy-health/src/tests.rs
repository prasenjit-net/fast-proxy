//! The health crate's behaviour end to end, against real sockets.

use std::sync::Arc;
use std::time::Duration;

use fast_proxy_config_service::ConfigChanged;
use fast_proxy_model::{HealthCheck, HealthKind};
use fast_proxy_routing::SnapshotHolder;
use fast_proxy_telemetry::{EndpointChange, EndpointEvents, Metrics};
use tokio::sync::{broadcast, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::prober::{run_prober, EndpointName, ProbeContext, ProberSpec};
use crate::reconciler::Reconciler;
use crate::status_gauges::run_status_gauges;
use crate::test_support::{config_for, eventually, fast_check, snapshot, Backend};
use crate::HealthEngine;

fn context(permits: usize) -> Arc<ProbeContext> {
    Arc::new(ProbeContext {
        metrics: Arc::new(Metrics::new()),
        events: EndpointEvents::new(),
        concurrency: Semaphore::new(permits),
    })
}

fn only_runtime(
    snapshot: &fast_proxy_routing::Snapshot,
) -> Arc<fast_proxy_balancing::EndpointRuntime> {
    let subsystem = snapshot.subsystems.values().next().unwrap();
    subsystem.upstreams.values().next().unwrap().candidates[0]
        .runtime
        .clone()
}

// ---- one prober ---------------------------------------------------------

/// How many probes with `result` ("ok" or "fail") the metrics have counted.
fn probes(metrics: &Metrics, result: &str) -> u64 {
    metrics
        .render()
        .lines()
        .filter(|line| {
            line.starts_with("fastproxy_health_checks_total")
                && line.contains(&format!("result=\"{result}\""))
        })
        .filter_map(|line| line.rsplit(' ').next()?.parse::<u64>().ok())
        .sum()
}

#[tokio::test]
async fn a_prober_flips_health_only_after_exactly_fall_failures_and_rise_successes() {
    let backend = Backend::start().await;
    let check = HealthCheck {
        rise: 3,
        fall: 3,
        ..fast_check("/healthz")
    };
    let config = config_for(check.clone(), &[backend.addr]);
    let snapshot = snapshot(&config, None);
    let runtime = only_runtime(&snapshot);
    let context = context(8);
    let mut events = context.events.subscribe();
    let cancel = CancellationToken::new();
    let spec = ProberSpec {
        name: EndpointName {
            subsystem: "shop".into(),
            upstream: "backend".into(),
            endpoint: backend.addr.to_string(),
        },
        addr: fast_proxy_model::EndpointAddr::parse(&backend.addr.to_string()).unwrap(),
        check,
        tls: None,
    };
    tokio::spawn(run_prober(
        spec,
        runtime.clone(),
        context.clone(),
        cancel.clone(),
    ));

    assert!(
        eventually(5, || probes(&context.metrics, "ok") >= 1).await,
        "probing started"
    );
    backend.set_status(500);
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(&event.change, EndpointChange::Unhealthy { reason } if reason.contains("500")),
        "{event:?}"
    );
    assert_eq!(event.endpoint, backend.addr.to_string());
    assert!(!runtime.is_active_healthy());
    assert!(
        matches!(runtime.last_probe(), Some((false, _))),
        "the latest probe is on record"
    );
    assert_eq!(
        probes(&context.metrics, "fail"),
        3,
        "flipped on the third failed probe"
    );

    let ok_before = probes(&context.metrics, "ok");
    backend.set_status(200);
    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.change, EndpointChange::Healthy);
    assert!(runtime.is_active_healthy());
    assert_eq!(
        probes(&context.metrics, "ok") - ok_before,
        3,
        "flipped on the third good probe"
    );
    cancel.cancel();
}

#[tokio::test]
async fn a_killed_backend_is_marked_unhealthy_and_a_revived_one_healthy() {
    let backend = Backend::start().await;
    let config = config_for(fast_check("/"), &[backend.addr]);
    let holder = Arc::new(SnapshotHolder::new());
    holder.swap(snapshot(&config, None));
    let runtime = only_runtime(&holder.load());
    let (_changes_tx, changes) = broadcast::channel::<ConfigChanged>(4);
    let shutdown = CancellationToken::new();
    let engine = HealthEngine::new(holder, Arc::new(Metrics::new()), EndpointEvents::new());
    tokio::spawn(engine.run(changes, shutdown.clone()));

    backend.kill();
    assert!(eventually(5, || !runtime.is_active_healthy()).await);
    backend.revive().await;
    assert!(eventually(5, || runtime.is_active_healthy()).await);
    shutdown.cancel();
}

#[tokio::test]
async fn probes_wait_for_a_permit_so_concurrency_is_bounded() {
    let backend = Backend::start().await;
    let config = config_for(fast_check("/"), &[backend.addr]);
    let snapshot = snapshot(&config, None);
    let mut reconciler = Reconciler::new(context(0));
    reconciler.reconcile(&snapshot);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(backend.probes_of("/"), 0, "no permit, no probe");
    reconciler.stop_all();
}

// ---- reconciling ----------------------------------------------------------

#[tokio::test]
async fn probers_exist_only_for_groups_with_checks_enabled() {
    let backend = Backend::start().await;
    let mut off = fast_check("/");
    off.kind = HealthKind::None;
    let mut reconciler = Reconciler::new(context(8));

    reconciler.reconcile(&snapshot(&config_for(off, &[backend.addr]), None));
    assert!(reconciler.endpoints().is_empty());

    reconciler.reconcile(&snapshot(
        &config_for(fast_check("/"), &[backend.addr]),
        None,
    ));
    assert_eq!(
        reconciler.endpoints(),
        [format!("backend/{}", backend.addr)]
    );
    reconciler.stop_all();
    assert!(reconciler.endpoints().is_empty());
}

#[tokio::test]
async fn unchanged_probers_keep_running_and_changed_or_removed_ones_are_replaced() {
    let (first, second) = (Backend::start().await, Backend::start().await);
    let mut reconciler = Reconciler::new(context(8));
    let before = snapshot(
        &config_for(fast_check("/a"), &[first.addr, second.addr]),
        None,
    );
    reconciler.reconcile(&before);
    let old_tokens = reconciler.tokens();
    assert_eq!(old_tokens.len(), 2);

    // The same configuration again: nothing restarts.
    let same = snapshot(
        &config_for(fast_check("/a"), &[first.addr, second.addr]),
        Some(&before),
    );
    reconciler.reconcile(&same);
    assert!(old_tokens.iter().all(|t| !t.is_cancelled()));

    // A different probe path restarts both.
    let moved = snapshot(
        &config_for(fast_check("/b"), &[first.addr, second.addr]),
        Some(&same),
    );
    reconciler.reconcile(&moved);
    assert!(old_tokens.iter().all(CancellationToken::is_cancelled));
    assert!(
        eventually(5, || first.probes_of("/b") > 0
            && second.probes_of("/b") > 0)
        .await
    );

    // Removing an endpoint stops only its prober.
    let kept = reconciler.tokens();
    let shrunk = snapshot(&config_for(fast_check("/b"), &[first.addr]), Some(&moved));
    reconciler.reconcile(&shrunk);
    assert_eq!(reconciler.endpoints(), [format!("backend/{}", first.addr)]);
    assert_eq!(kept.iter().filter(|t| t.is_cancelled()).count(), 1);
    reconciler.stop_all();
}

#[tokio::test]
async fn the_engine_follows_configuration_changes_and_stops_at_shutdown() {
    let (first, second) = (Backend::start().await, Backend::start().await);
    let holder = Arc::new(SnapshotHolder::new());
    holder.swap(snapshot(&config_for(fast_check("/"), &[first.addr]), None));
    let (changes_tx, changes) = broadcast::channel(4);
    let shutdown = CancellationToken::new();
    let engine = HealthEngine::new(
        holder.clone(),
        Arc::new(Metrics::new()),
        EndpointEvents::new(),
    );
    let running = tokio::spawn(engine.run(changes, shutdown.clone()));

    assert!(
        eventually(5, || first.probes_of("/") > 0).await,
        "initial prober runs"
    );
    assert_eq!(second.probes_of("/"), 0);

    let next = snapshot(
        &config_for(fast_check("/"), &[first.addr, second.addr]),
        Some(&holder.load()),
    );
    holder.swap(next);
    changes_tx
        .send(ConfigChanged {
            version: 2,
            summary: "add".into(),
        })
        .unwrap();
    assert!(
        eventually(5, || second.probes_of("/") > 0).await,
        "new endpoint is probed"
    );

    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (a, b) = (first.probes_of("/"), second.probes_of("/"));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        (first.probes_of("/"), second.probes_of("/")),
        (a, b),
        "no probes after shutdown"
    );
}

#[tokio::test]
async fn the_engine_ends_when_the_change_channel_closes() {
    let holder = Arc::new(SnapshotHolder::new());
    let (changes_tx, changes) = broadcast::channel::<ConfigChanged>(4);
    let engine = HealthEngine::new(holder, Arc::new(Metrics::new()), EndpointEvents::new());
    let running = tokio::spawn(engine.run(changes, CancellationToken::new()));
    drop(changes_tx);
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .unwrap()
        .unwrap();
}

// ---- status gauges --------------------------------------------------------

#[tokio::test]
async fn gauges_show_health_ejection_and_in_flight_and_forget_removed_endpoints() {
    let (up, down) = (Backend::start().await, Backend::start().await);
    let holder = Arc::new(SnapshotHolder::new());
    let config = config_for(fast_check("/"), &[up.addr, down.addr]);
    let live = snapshot(&config, None);
    let subsystem = live.subsystems.values().next().unwrap();
    let candidates = &subsystem.upstreams.values().next().unwrap().candidates;
    candidates[1].runtime.record_probe(false, 1, 1);
    let _busy = (0..3)
        .map(|_| candidates[0].runtime.track())
        .collect::<Vec<_>>();
    holder.swap(live.clone());

    let metrics = Arc::new(Metrics::new());
    let shutdown = CancellationToken::new();
    tokio::spawn(run_status_gauges(
        holder.clone(),
        metrics.clone(),
        shutdown.clone(),
    ));
    let healthy = |addr: &std::net::SocketAddr, value: u8| {
        format!(
            r#"fastproxy_upstream_endpoint_healthy{{endpoint="{addr}",subsystem="shop",upstream="backend"}} {value}"#
        )
    };
    assert!(eventually(3, || metrics.render().contains(&healthy(&up.addr, 1))).await);
    let text = metrics.render();
    assert!(text.contains(&healthy(&down.addr, 0)), "{text}");
    assert!(text.contains(&format!(
        r#"fastproxy_upstream_endpoint_in_flight{{endpoint="{}",subsystem="shop",upstream="backend"}} 3"#,
        up.addr
    )));

    holder.swap(snapshot(
        &config_for(fast_check("/"), &[up.addr]),
        Some(&live),
    ));
    assert!(
        eventually(3, || !metrics.render().contains(&down.addr.to_string())).await,
        "the removed endpoint's series are deleted"
    );
    shutdown.cancel();
}
