//! Assembles the whole proxy: storage, the config pipeline, the data plane
//! listeners, and the admin plane, then runs until a shutdown signal.

use std::net::SocketAddr;
use std::sync::Arc;

use fast_proxy_admin::{AdminState, ListenerInfo, RuntimeInfo, TlsInfo};
use fast_proxy_config_service::{ConfigService, StartError, TlsSettings};
use fast_proxy_dataplane::{
    bind, BindError, BoundListener, DataPlane, ListenerSpec, Observability, RequestPipeline,
    TlsPlane, TlsPlaneError, TlsPlaneSettings, TrustedProxies, TrustedProxyError, UpstreamClient,
};
use fast_proxy_health::{run_status_gauges, HealthEngine};
use fast_proxy_store::{Store, StoreError};
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use fast_proxy_tls::{MinTlsVersion, ServerTlsPolicy};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::AppConfig;

/// Anything that stops the proxy from starting or keeps it from serving.
#[derive(Debug, Error)]
pub enum StartupError {
    #[error(transparent)]
    TrustedProxies(#[from] TrustedProxyError),
    #[error("cannot open the database: {0}")]
    Store(#[from] StoreError),
    #[error(transparent)]
    StoredConfig(#[from] StartError),
    #[error(transparent)]
    DataPlaneBind(#[from] BindError),
    #[error(transparent)]
    TlsPlane(#[from] TlsPlaneError),
    #[error("cannot bind the admin plane on {addr}: {source}")]
    AdminBind {
        addr: SocketAddr,
        source: std::io::Error,
    },
    #[error("the admin server failed: {0}")]
    AdminServe(std::io::Error),
}

/// Starts everything and serves until Ctrl-C. Every listener is bound
/// before any traffic is served, so a bind failure aborts startup cleanly.
pub async fn run(config: AppConfig) -> Result<(), StartupError> {
    let data_plane = &config.data_plane;
    let trusted_proxies = TrustedProxies::parse(&data_plane.trusted_proxies)?;

    let database = data_plane.database_path();
    let store = Store::open(&database)?;
    // Both planes report to the same metrics and the same access log.
    let metrics = Arc::new(Metrics::new());
    let config_service = Arc::new(
        ConfigService::start_with_tls(store, metrics.clone(), tls_settings(&config)).await?,
    );

    let specs: Vec<ListenerSpec> = data_plane
        .listener
        .iter()
        .map(|listener| ListenerSpec {
            name: listener.name.clone(),
            addr: listener.listen,
        })
        .collect();
    let bound = bind(&specs).await?;
    let https = bind_https(&config, &config_service, &metrics).await?;
    let admin_addr = config.admin.listen;
    let admin_listener =
        TcpListener::bind(admin_addr)
            .await
            .map_err(|source| StartupError::AdminBind {
                addr: admin_addr,
                source,
            })?;

    let access_log = Arc::new(AccessLog::open(config.logging.access_log.as_deref()).await);
    let runtime = RuntimeInfo {
        listeners: bound
            .iter()
            .chain(https.iter().map(|plane| plane.listener()))
            .map(|listener| ListenerInfo {
                name: listener.name.clone(),
                addr: listener.local_addr(),
            })
            .collect(),
        database_path: database.display().to_string(),
        tls: TlsInfo {
            enabled: config.data_plane.tls.enabled,
            listen: https.as_ref().map(|plane| plane.listener().local_addr()),
            // HTTP/2 stays off until the data plane serves it (plan task D2).
            http2: false,
            min_version: match config.data_plane.tls.min_version {
                MinTlsVersion::V1_2 => "1.2",
                MinTlsVersion::V1_3 => "1.3",
            }
            .to_string(),
        },
    };
    // Shutdown happens in two stages: the signal stops the data plane, and
    // only once it has drained does the admin plane stop (so the admin API
    // stays usable while requests finish).
    let data_stop = CancellationToken::new();
    let admin_stop = CancellationToken::new();
    // Endpoint health changes flow from the data plane and the health checks to
    // the admin plane's live view.
    let endpoint_events = EndpointEvents::new();
    let admin_state = Arc::new(AdminState::new(
        config.ui,
        env!("CARGO_PKG_VERSION"),
        access_log.clone(),
        config_service.clone(),
        runtime,
        metrics.clone(),
        endpoint_events.clone(),
    ));
    fast_proxy_admin::spawn_background_tasks(admin_state.clone());
    tokio::spawn(cancel_on_ctrl_c(data_stop.clone()));

    // Active health checks and the endpoint gauges follow the live
    // configuration; they stop with the data plane.
    let health = HealthEngine::new(
        config_service.snapshot_holder(),
        metrics.clone(),
        endpoint_events.clone(),
    );
    tokio::spawn(health.run(config_service.subscribe(), data_stop.clone()));
    // Changes written by another instance sharing the database are picked up
    // here; a zero interval turns the sync off.
    tokio::spawn(
        config_service
            .clone()
            .run_sync(config.data_plane.config_poll_interval, data_stop.clone()),
    );
    tokio::spawn(run_status_gauges(
        config_service.snapshot_holder(),
        metrics.clone(),
        data_stop.clone(),
    ));

    let pipeline = Arc::new(RequestPipeline::new(
        config_service.snapshot_holder(),
        UpstreamClient::new(trusted_proxies, metrics.clone(), endpoint_events),
        Observability {
            metrics,
            access_log: access_log.clone(),
        },
    ));
    let plane = DataPlane::start_with_tls(bound, https, pipeline, data_stop.clone());

    for (name, addr) in plane.local_addrs() {
        let scheme = if name == HTTPS_LISTENER {
            "https"
        } else {
            "http"
        };
        tracing::info!("data plane listener \"{name}\" serving on {scheme}://{addr}");
    }
    tracing::info!(
        "admin plane serving on http://{admin_addr}, database {}, config version {}",
        database.display(),
        config_service.snapshot_holder().load().version
    );
    if let Some(warning) = admin_exposure_warning(admin_addr) {
        tracing::warn!("{warning}");
    }

    let grace = config.data_plane.shutdown_grace;
    let drain = tokio::spawn({
        let admin_stop = admin_stop.clone();
        async move {
            let report = plane.stopped(grace).await;
            admin_stop.cancel();
            report
        }
    });

    let served = axum::serve(
        admin_listener,
        fast_proxy_admin::router(admin_state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(admin_stop.cancelled_owned())
    .await;
    // However the admin server ended, stop the data plane with it.
    data_stop.cancel();
    if let Ok(report) = drain.await {
        if report.forced_connections > 0 {
            tracing::warn!(
                "closed {} connection(s) still open after the {grace:?} grace period",
                report.forced_connections
            );
        }
    }
    // Every request finishes before this point; make sure its line is on disk.
    access_log.flush().await;
    tracing::info!("stopped");
    served.map_err(StartupError::AdminServe)
}

/// The name of the HTTPS listener in logs, metrics and the admin runtime view.
const HTTPS_LISTENER: &str = "https";

/// What the config service needs to know about TLS: the switch, and where the
/// master key comes from (`FAST_PROXY_MASTER_KEY` wins over the file).
fn tls_settings(config: &AppConfig) -> TlsSettings {
    TlsSettings {
        enabled: config.data_plane.tls.enabled,
        master_key_file: Some(config.data_plane.master_key_path()),
        env_master_key: std::env::var("FAST_PROXY_MASTER_KEY").ok(),
    }
}

/// Binds the HTTPS listener and prepares its handshakes when TLS is on.
/// HTTP/2 stays off until the data plane serves it (plan task D2).
async fn bind_https(
    config: &AppConfig,
    config_service: &ConfigService,
    metrics: &Arc<Metrics>,
) -> Result<Option<TlsPlane>, StartupError> {
    let tls = &config.data_plane.tls;
    if !tls.enabled {
        return Ok(None);
    }
    let spec = ListenerSpec {
        name: HTTPS_LISTENER.into(),
        addr: tls.listen,
    };
    let listener: BoundListener = bind(&[spec]).await?.remove(0);
    let plane = TlsPlane::new(
        listener,
        config_service.snapshot_holder(),
        TlsPlaneSettings {
            policy: ServerTlsPolicy {
                min_version: tls.min_version,
                http2: false,
            },
            handshake_timeout: tls.handshake_timeout,
            max_concurrent_handshakes: tls.max_concurrent_handshakes,
            reject_unknown_sni: tls.reject_unknown_sni,
        },
        metrics.clone(),
    )?;
    Ok(Some(plane))
}

async fn cancel_on_ctrl_c(shutdown: CancellationToken) {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("shutting down");
    }
    shutdown.cancel();
}

/// The admin plane has no authentication until M5, so anything but a
/// loopback bind deserves a loud warning.
fn admin_exposure_warning(addr: SocketAddr) -> Option<String> {
    if addr.ip().is_loopback() {
        return None;
    }
    Some(format!(
        "the admin plane is listening on {addr}, which is not a loopback address, \
         and it has NO authentication: anyone who can reach it can reconfigure the proxy"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_admin_addresses_are_not_warned_about() {
        assert!(admin_exposure_warning("127.0.0.1:9000".parse().unwrap()).is_none());
        assert!(admin_exposure_warning("[::1]:9000".parse().unwrap()).is_none());
    }

    #[test]
    fn exposed_admin_addresses_get_a_warning_naming_the_address() {
        for addr in ["0.0.0.0:9000", "192.168.1.5:9000", "[::]:9000"] {
            let warning = admin_exposure_warning(addr.parse().unwrap()).unwrap();
            assert!(warning.contains(addr), "{warning}");
            assert!(warning.contains("NO authentication"));
        }
    }
}
