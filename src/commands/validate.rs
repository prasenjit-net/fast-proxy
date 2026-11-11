//! `config validate`: check the configuration the next start would use, and
//! optionally the configuration stored in the database, without starting
//! anything and without creating a file or directory.

use std::path::{Path, PathBuf};

use fast_proxy_balancing::RuntimeRegistry;
use fast_proxy_routing::{compile_all, MaintenanceInputs, TlsInputs};
use fast_proxy_store::repository::maintenance;
use fast_proxy_store::{Database, StoreError};

use crate::cli::Cli;
use crate::commands::failure::{Failure, Outcome};
use crate::config::AppConfig;

/// Validates the configuration file (with the command-line overrides applied)
/// and, when asked, the stored configuration at `database`.
pub fn run(cli: &Cli, check_database: Option<&Option<PathBuf>>) -> Outcome {
    if !cli.config.exists() {
        return Err(Failure::Usage(format!(
            "the configuration file {} does not exist",
            cli.config.display()
        )));
    }
    let mut config =
        AppConfig::load(&cli.config).map_err(|err| Failure::Refused(err.to_string()))?;
    cli.apply(&mut config);
    config
        .check()
        .map_err(|err| Failure::Refused(err.to_string()))?;
    let mut out = format!("the configuration {} is valid", cli.config.display());
    if let Some(requested) = check_database {
        let path = requested
            .clone()
            .unwrap_or_else(|| config.data_plane.database_path());
        out.push('\n');
        out.push_str(&check_stored(&path)?);
    }
    Ok(out)
}

/// Opens the database read-only and compiles what it stores. Certificates stay
/// sealed (no master key is read), so HTTPS settings are not checked against
/// them, which the answer says.
fn check_stored(path: &Path) -> Outcome {
    if !path.exists() {
        return Err(Failure::Usage(format!("no database at {}", path.display())));
    }
    let db = Database::open_read_only(path).map_err(|err| match err {
        StoreError::NotFound(what) => Failure::Refused(format!("{} is not {what}", path.display())),
        other => Failure::Refused(format!("cannot read {}: {other}", path.display())),
    })?;
    let stored = db.load_all().map_err(refused)?;
    let version = db.current_version().map_err(refused)?;
    let scopes = db
        .read(maintenance::list)
        .map_err(refused)?
        .into_iter()
        .map(|row| (row.target, row.settings))
        .collect();
    let snapshot = compile_all(
        version,
        &stored,
        &RuntimeRegistry::new(),
        &TlsInputs::off(),
        &MaintenanceInputs {
            scopes,
            now_ms: now_ms(),
        },
    )
    .map_err(|err| {
        Failure::Refused(format!(
            "the stored configuration (version {version}) does not compile: {err}"
        ))
    })?;
    let routes: usize = snapshot.subsystems.values().map(|s| s.routes.len()).sum();
    Ok(format!(
        "the stored configuration (version {version}) compiles: {} subsystems, {routes} routes\n\
         note: certificates are not decrypted here, so HTTPS settings are not checked against them",
        snapshot.subsystems.len()
    ))
}

fn refused(err: StoreError) -> Failure {
    Failure::Refused(err.to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}
