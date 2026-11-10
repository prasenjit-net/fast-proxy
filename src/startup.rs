//! Assembles the whole proxy: storage, the config pipeline, the data plane
//! listeners, and the admin plane, then runs until a shutdown signal.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use fast_proxy_admin::{
    serve_tls, AdminSecurity, AdminState, ListenerInfo, RuntimeInfo, SecuritySettings,
    StoredCertificate, TlsInfo,
};
use fast_proxy_config_service::{
    AuditRetention, Bootstrap, ChangeContext, ConfigService, StartError, TlsSettings,
};
use fast_proxy_dataplane::{
    bind, BindError, BoundListener, DataPlane, Http2Limits, ListenerSpec, Observability,
    RequestPipeline, TlsPlane, TlsPlaneError, TlsPlaneSettings, TrustedProxies, TrustedProxyError,
    UpstreamClient,
};
use fast_proxy_health::{run_status_gauges, HealthEngine};
use fast_proxy_store::{Store, StoreError};
use fast_proxy_telemetry::{AccessLog, EndpointEvents, Metrics};
use fast_proxy_tls::{build_server_config, MinTlsVersion, ServerConfigError, ServerTlsPolicy};
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::config::{AdminAuth, AdminConfig, AdminTlsConfig, AppConfig, TlsConfig};

/// The environment variable that seeds the first admin token (for containers).
const BOOTSTRAP_TOKEN_VARIABLE: &str = "FAST_PROXY_BOOTSTRAP_TOKEN";
/// How often expired maintenance scopes are cleared.
const MAINTENANCE_SWEEP_INTERVAL: Duration = Duration::from_secs(1);
/// How much audit history is kept until `[admin.audit]` settings exist.
const AUDIT_RETENTION: AuditRetention = AuditRetention {
    max_age: Duration::from_secs(90 * 86_400),
    max_rows: 200_000,
};
/// How often the audit log is pruned.
const AUDIT_PRUNE_INTERVAL: Duration = Duration::from_secs(3_600);

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
    #[error(transparent)]
    AdminTls(#[from] ServerConfigError),
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
    if config.admin.auth == AdminAuth::Required {
        ensure_admin_token(&config_service).await?;
    }
    if config.admin.tls.enabled {
        ensure_admin_certificate(&config_service, &config.admin.tls).await?;
    }

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
            http2: config.data_plane.tls.http2,
            redirect_port: redirect_port(&config.data_plane.tls, https.as_ref()),
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
    let security = AdminSecurity::new(
        &SecuritySettings {
            auth_required: config.admin.auth == AdminAuth::Required,
            channel_protected: admin_addr.ip().is_loopback() || config.admin.tls.enabled,
            listen: admin_addr,
            allowed_hosts: config.admin.allowed_hosts.clone(),
            admin_certificate: config
                .admin
                .tls
                .enabled
                .then(|| config.admin.tls.certificate.clone()),
        },
        config_service.token_cache(),
    );
    let admin_state = Arc::new(AdminState::new(
        config.ui,
        env!("CARGO_PKG_VERSION"),
        access_log.clone(),
        config_service.clone(),
        runtime,
        metrics.clone(),
        endpoint_events.clone(),
        security,
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
    // Expired maintenance is cleared, and the audit log kept bounded, in the
    // background; both stop with the data plane.
    tokio::spawn(
        config_service
            .clone()
            .run_maintenance_sweeper(MAINTENANCE_SWEEP_INTERVAL, data_stop.clone()),
    );
    tokio::spawn(config_service.clone().run_audit_pruner(
        AUDIT_RETENTION,
        AUDIT_PRUNE_INTERVAL,
        data_stop.clone(),
    ));
    tokio::spawn(run_status_gauges(
        config_service.snapshot_holder(),
        metrics.clone(),
        data_stop.clone(),
    ));

    let pipeline = Arc::new(
        RequestPipeline::new(
            config_service.snapshot_holder(),
            UpstreamClient::new(trusted_proxies.clone(), metrics.clone(), endpoint_events),
            Observability {
                metrics,
                access_log: access_log.clone(),
            },
        )
        .with_https_port(redirect_port(&config.data_plane.tls, https.as_ref()))
        .with_trusted_proxies(trusted_proxies),
    );
    let plane = DataPlane::start_with_tls(bound, https, pipeline, data_stop.clone());

    for (name, addr) in plane.local_addrs() {
        let scheme = if name == HTTPS_LISTENER {
            "https"
        } else {
            "http"
        };
        tracing::info!("data plane listener \"{name}\" serving on {scheme}://{addr}");
    }
    let admin_scheme = if config.admin.tls.enabled {
        "https"
    } else {
        "http"
    };
    tracing::info!(
        "admin plane serving on {admin_scheme}://{admin_addr}, database {}, config version {}",
        database.display(),
        config_service.snapshot_holder().load().version
    );
    for warning in admin_warnings(&config.admin) {
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

    let router = fast_proxy_admin::router(admin_state);
    let served = if config.admin.tls.enabled {
        let resolver = Arc::new(StoredCertificate::new(
            config_service.clone(),
            config.admin.tls.certificate.clone(),
        ));
        let tls = build_server_config(
            &ServerTlsPolicy {
                min_version: config.data_plane.tls.min_version,
                http2: true,
            },
            resolver,
        )?;
        serve_tls(admin_listener, router, Arc::new(tls), admin_stop.clone()).await;
        Ok(())
    } else {
        axum::serve(
            admin_listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(admin_stop.cancelled_owned())
        .await
    };
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

/// The port a redirect to HTTPS names: `public_port` when set (NAT, containers),
/// else the port the HTTPS listener is bound to, else the default 443.
fn redirect_port(tls: &TlsConfig, https: Option<&TlsPlane>) -> u16 {
    tls.public_port
        .or_else(|| https.map(|plane| plane.listener().local_addr().port()))
        .unwrap_or(443)
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
                http2: tls.http2,
            },
            handshake_timeout: tls.handshake_timeout,
            max_concurrent_handshakes: tls.max_concurrent_handshakes,
            reject_unknown_sni: tls.reject_unknown_sni,
            http2: Http2Limits {
                max_concurrent_streams: tls.http2_max_concurrent_streams,
                max_header_list_size: tls.http2_max_header_list_size,
                keep_alive_interval: tls.http2_keep_alive_interval,
                keep_alive_timeout: tls.http2_keep_alive_timeout,
            },
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

/// Makes sure the admin plane is never open and never locked out: with no
/// usable admin token, stores the one in `FAST_PROXY_BOOTSTRAP_TOKEN` or
/// generates one and prints it, once, on the console. The log gets only the
/// marker `admin token created`, never the token.
async fn ensure_admin_token(config_service: &ConfigService) -> Result<(), StoreError> {
    let seed = std::env::var(BOOTSTRAP_TOKEN_VARIABLE).ok();
    match config_service.ensure_admin_token(seed.as_deref()).await? {
        Bootstrap::Created(token) => {
            tracing::info!("admin token created");
            println!("admin token (shown once, store it now): {}", token.reveal());
        }
        Bootstrap::Seeded => {
            tracing::info!("admin token created from {BOOTSTRAP_TOKEN_VARIABLE}");
        }
        Bootstrap::NotNeeded => {}
    }
    Ok(())
}

/// Makes sure the stored certificate the admin plane serves exists, generating
/// a self-signed one for the configured names the first time (browsers will warn
/// about it until an uploaded certificate of the same name replaces it).
async fn ensure_admin_certificate(
    config_service: &ConfigService,
    tls: &AdminTlsConfig,
) -> Result<(), StoreError> {
    let generated = config_service
        .ensure_self_signed_certificate(
            &ChangeContext::system(),
            &tls.certificate,
            tls.names.clone(),
        )
        .await?;
    if generated {
        tracing::info!(
            "generated a self-signed certificate \"{}\" for the admin plane; upload one of the \
             same name to replace it",
            tls.certificate
        );
    }
    Ok(())
}

/// Warnings for admin settings that are allowed but worth saying out loud.
fn admin_warnings(admin: &AdminConfig) -> Vec<String> {
    let mut warnings = Vec::new();
    if admin.auth == AdminAuth::Off {
        warnings.push(
            "admin authentication is OFF: anyone who can reach the admin plane can reconfigure \
             the proxy (allowed only because it listens on loopback)"
                .to_string(),
        );
    }
    if !admin.listen.ip().is_loopback() && !admin.tls.enabled {
        warnings.push(format!(
            "the admin plane is listening on {}, which is not a loopback address, without TLS \
             (admin.allow_insecure_remote): admin tokens cross the network in clear text",
            admin.listen
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admin(listen: &str, auth: AdminAuth) -> AdminConfig {
        AdminConfig {
            listen: listen.parse().unwrap(),
            auth,
            allow_insecure_remote: true,
            ..AdminConfig::default()
        }
    }

    #[test]
    fn a_protected_loopback_admin_plane_has_nothing_to_warn_about() {
        assert!(admin_warnings(&admin("127.0.0.1:9000", AdminAuth::Required)).is_empty());
        assert!(admin_warnings(&admin("[::1]:9000", AdminAuth::Required)).is_empty());
    }

    #[test]
    fn an_exposed_admin_address_with_tls_has_nothing_to_warn_about() {
        let mut config = admin("0.0.0.0:9000", AdminAuth::Required);
        config.tls.enabled = true;
        assert!(admin_warnings(&config).is_empty());
    }

    #[test]
    fn switching_authentication_off_is_warned_about_every_start() {
        let warnings = admin_warnings(&admin("127.0.0.1:9000", AdminAuth::Off));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("authentication is OFF"));
    }

    #[test]
    fn an_exposed_admin_address_without_tls_is_warned_about_naming_the_address() {
        for addr in ["0.0.0.0:9000", "192.168.1.5:9000", "[::]:9000"] {
            let warnings = admin_warnings(&admin(addr, AdminAuth::Required));
            assert_eq!(warnings.len(), 1, "{addr}");
            assert!(warnings[0].contains(addr));
            assert!(warnings[0].contains("clear text"));
        }
    }
}
