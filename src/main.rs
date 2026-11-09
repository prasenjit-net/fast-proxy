mod access_log;
mod config;
mod error;
mod routes;
mod services;
mod state;
mod static_assets;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::config::AppConfig;
use crate::state::AppState;

/// Single-binary fullstack template: Axum REST API + embedded React SPA.
///
/// Configuration is read from a TOML file; every flag below overrides
/// the corresponding value from that file.
#[derive(Parser, Debug)]
#[command(name = "rusty-template", version, about)]
struct Cli {
    /// Path to the TOML configuration file
    #[arg(short, long, default_value = "config.toml")]
    config: PathBuf,

    /// Override [server].host
    #[arg(long)]
    host: Option<String>,

    /// Override [server].port
    #[arg(short, long)]
    port: Option<u16>,

    /// Override [logging].level (trace, debug, info, warn, error)
    #[arg(long)]
    log_level: Option<String>,

    /// Override [logging].access_log file path
    #[arg(long)]
    access_log: Option<PathBuf>,

    /// Disable the access log file entirely
    #[arg(long)]
    no_access_log: bool,
}

impl Cli {
    fn apply(&self, config: &mut AppConfig) {
        if let Some(host) = &self.host {
            config.server.host = host.clone();
        }
        if let Some(port) = self.port {
            config.server.port = port;
        }
        if let Some(level) = &self.log_level {
            config.logging.level = level.clone();
        }
        if let Some(path) = &self.access_log {
            config.logging.access_log = Some(path.clone());
        }
        if self.no_access_log {
            config.logging.access_log = None;
        }
    }
}

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
    let addr = format!("{}:{}", config.server.host, config.server.port);
    let state = Arc::new(AppState::new(config).await);
    services::metrics::spawn(state.clone());

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on http://{addr}");

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
