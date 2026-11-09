//! The `fast-proxy` binary: CLI parsing, configuration, logging, and server startup.

mod cli;
mod config;

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use fast_proxy_admin::AdminState;
use fast_proxy_telemetry::AccessLog;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::config::AppConfig;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    let mut config = match AppConfig::load(&cli.config) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    cli.apply(&mut config);
    if let Err(err) = config.validate() {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    let filter =
        EnvFilter::try_new(&config.logging.level).unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    if let Err(err) = run(config).await {
        tracing::error!("fatal: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn run(config: AppConfig) -> Result<(), Box<dyn std::error::Error>> {
    log_data_plane_summary(&config);
    let addr = config.admin.listen;
    let access_log = AccessLog::open(config.logging.access_log.as_deref()).await;
    let state = Arc::new(AdminState::new(
        config.ui,
        env!("CARGO_PKG_VERSION"),
        access_log,
    ));
    fast_proxy_admin::spawn_sampler(state.clone());

    let app = fast_proxy_admin::router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("admin plane listening on http://{addr}");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await?;

    Ok(())
}

/// Logs what the data plane will use. The listeners are bound once the
/// proxy lands (M1 task D1); until then this only reports the config.
fn log_data_plane_summary(config: &AppConfig) {
    let data_plane = &config.data_plane;
    for listener in &data_plane.listener {
        tracing::info!(
            "data plane listener \"{}\" configured on {} (not bound yet)",
            listener.name,
            listener.listen
        );
    }
    tracing::info!(
        "database {}, shutdown grace {:?}, {} trusted proxy CIDRs",
        data_plane.database_path().display(),
        data_plane.shutdown_grace,
        data_plane.trusted_proxies.len()
    );
}
