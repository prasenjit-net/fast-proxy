//! The `fast-proxy` binary: CLI parsing, configuration, logging, and server startup.

mod cli;
mod commands;
mod config;
mod startup;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;
use crate::config::AppConfig;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(command) = &cli.command {
        return commands::run(&cli, command).await;
    }

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

    if let Err(err) = startup::run(config).await {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
