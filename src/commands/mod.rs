//! The subcommands (ADR 0019): operations against a running proxy, `config
//! validate`, and offline token management. Each prints its result and ends
//! with the exit code of [`Failure`]: 0 success, 1 refused or invalid, 2 usage
//! or connection problems.

mod client;
mod credentials;
mod failure;
mod remote;
mod tokens;
mod validate;

use std::process::ExitCode;

use crate::cli::{Cli, Command, ConfigAction, MaintenanceAction, RemoteArgs, TokenAction};
use crate::config::AppConfig;
use client::{AdminClient, Trust};
use failure::{Failure, Outcome};

/// Runs `command` and returns the process's exit code.
pub async fn run(cli: &Cli, command: &Command) -> ExitCode {
    match execute(cli, command).await {
        Ok(text) => {
            if !text.is_empty() {
                println!("{text}");
            }
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("error: {}", failure.message());
            failure.exit_code()
        }
    }
}

async fn execute(cli: &Cli, command: &Command) -> Outcome {
    match command {
        Command::Maintenance { action } => match action {
            MaintenanceAction::Status(remote) => {
                remote::maintenance_status(&connect(cli, remote)?, remote.json).await
            }
            MaintenanceAction::On(on) => {
                remote::maintenance_on(&connect(cli, &on.remote)?, on).await
            }
            MaintenanceAction::Off(off) => {
                remote::maintenance_off(&connect(cli, &off.remote)?, off).await
            }
        },
        Command::Reload(remote) => remote::reload(&connect(cli, remote)?, remote.json).await,
        Command::Status(remote) => remote::status(&connect(cli, remote)?, remote.json).await,
        Command::Audit(args) => remote::audit(&connect(cli, &args.remote)?, args).await,
        Command::Config { action } => match action {
            ConfigAction::Validate(args) => validate::run(cli, args.check_database.as_ref()),
        },
        Command::Token { action } => {
            let database = database_path(cli)?;
            let now = now_ms();
            match action {
                TokenAction::Create(args) => tokens::create(&database, args, now),
                TokenAction::List => tokens::list(&database),
                TokenAction::Revoke { token } => tokens::revoke(&database, token, now),
            }
        }
    }
}

/// A client for the admin plane the arguments name, or the one the config
/// file's `[admin]` section describes.
fn connect(cli: &Cli, remote: &RemoteArgs) -> Result<AdminClient, Failure> {
    let url = match &remote.url {
        Some(url) => url.clone(),
        None => credentials::default_url(&loaded_config(cli)?),
    };
    let token = credentials::resolve_token(
        remote.token.as_deref(),
        remote.token_file.as_deref(),
        std::env::var(credentials::TOKEN_VARIABLE).ok().as_deref(),
    )?;
    let trust = Trust::from_flags(remote.ca_file.as_deref(), remote.insecure)?;
    AdminClient::new(&url, token, &trust)
}

/// The database file `token` works on: `--database`, else the configured one.
fn database_path(cli: &Cli) -> Result<std::path::PathBuf, Failure> {
    if let Some(path) = &cli.database {
        return Ok(path.clone());
    }
    Ok(loaded_config(cli)?.data_plane.database_path())
}

fn loaded_config(cli: &Cli) -> Result<AppConfig, Failure> {
    let mut config = AppConfig::load(&cli.config).map_err(|err| Failure::Usage(err.to_string()))?;
    cli.apply(&mut config);
    Ok(config)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
