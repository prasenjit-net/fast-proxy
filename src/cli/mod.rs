//! Command-line flags and how they override the config file.

mod subcommands;

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use crate::config::{AdminAuth, AppConfig};

pub use subcommands::{
    AuditArgs, Command, ConfigAction, MaintenanceAction, MaintenanceOff, MaintenanceOn, RemoteArgs,
    ScopeArgs, TokenAction, TokenCreate,
};

/// Fast, realtime-configurable ingress server and reverse proxy with an embedded admin UI.
///
/// Configuration is read from a TOML file; every flag below overrides
/// the corresponding value from that file.
#[derive(Parser, Debug)]
#[command(name = "fast-proxy", version, about)]
pub struct Cli {
    /// Do something other than serve: operate a running proxy, validate configuration,
    /// or manage tokens offline
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Path to the TOML configuration file
    #[arg(short, long, default_value = "config.toml", global = true)]
    pub config: PathBuf,

    /// Override [admin].listen (for example 127.0.0.1:9000)
    #[arg(long)]
    pub admin_listen: Option<SocketAddr>,

    /// Override [admin].auth (`off` is accepted only on a loopback admin address)
    #[arg(long, value_enum)]
    pub admin_auth: Option<AdminAuth>,

    /// Serve the admin plane over HTTPS (overrides [admin.tls].enabled)
    #[arg(long)]
    pub admin_tls: bool,

    /// Override [data_plane].database (also the file `token` works on)
    #[arg(long, global = true)]
    pub database: Option<PathBuf>,

    /// Override [data_plane].state_dir
    #[arg(long)]
    pub state_dir: Option<PathBuf>,

    /// Override [logging].level (trace, debug, info, warn, error)
    #[arg(long)]
    pub log_level: Option<String>,

    /// Override [logging].access_log file path
    #[arg(long)]
    pub access_log: Option<PathBuf>,

    /// Disable the access log file entirely
    #[arg(long)]
    pub no_access_log: bool,

    /// Turn TLS on, overriding [data_plane.tls].enabled
    #[arg(long, conflicts_with = "no_tls")]
    pub tls: bool,

    /// Turn TLS off, overriding [data_plane.tls].enabled
    #[arg(long)]
    pub no_tls: bool,

    /// Override [data_plane.tls].listen, the HTTPS address (for example 0.0.0.0:443)
    #[arg(long)]
    pub https_listen: Option<SocketAddr>,
}

impl Cli {
    /// Applies every flag that was given on top of the loaded config.
    pub fn apply(&self, config: &mut AppConfig) {
        if let Some(listen) = self.admin_listen {
            config.admin.listen = listen;
        }
        if self.admin_tls {
            config.admin.tls.enabled = true;
        }
        if let Some(auth) = self.admin_auth {
            config.admin.auth = auth;
        }
        if let Some(path) = &self.database {
            config.data_plane.database = Some(path.clone());
        }
        if let Some(dir) = &self.state_dir {
            config.data_plane.state_dir = dir.clone();
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
        if self.tls {
            config.data_plane.tls.enabled = true;
        }
        if self.no_tls {
            config.data_plane.tls.enabled = false;
        }
        if let Some(listen) = self.https_listen {
            config.data_plane.tls.listen = listen;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("fast-proxy").chain(args.iter().copied())).unwrap()
    }

    fn try_parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("fast-proxy").chain(args.iter().copied()))
    }

    #[test]
    fn the_command_definitions_are_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn no_subcommand_means_serve() {
        assert!(parse(&[]).command.is_none());
        assert!(parse(&["--admin-auth", "off"]).command.is_none());
    }

    #[test]
    fn maintenance_on_takes_scope_ttl_and_bypass() {
        let cli = parse(&[
            "maintenance",
            "on",
            "--subsystem",
            "shop",
            "--route",
            "/api",
            "--ttl",
            "30m",
            "--message",
            "deploying",
            "--retry-after",
            "90",
            "--bypass-cidr",
            "10.0.0.0/8",
            "--bypass-cidr",
            "192.0.2.0/24",
            "--bypass-header",
            "X-Pass",
        ]);
        let Some(Command::Maintenance {
            action: MaintenanceAction::On(on),
        }) = cli.command
        else {
            panic!("maintenance on")
        };
        assert_eq!(on.scope.subsystem.as_deref(), Some("shop"));
        assert_eq!(on.scope.route.as_deref(), Some("/api"));
        assert_eq!(on.ttl, Some(std::time::Duration::from_secs(1800)));
        assert_eq!(on.retry_after, Some(90));
        assert_eq!(on.bypass_cidrs, ["10.0.0.0/8", "192.0.2.0/24"]);
        assert_eq!(on.bypass_header.as_deref(), Some("X-Pass"));
    }

    #[test]
    fn dependent_flags_need_their_partner() {
        assert!(try_parse(&["maintenance", "on", "--route", "/api"]).is_err());
        assert!(try_parse(&["maintenance", "on", "--bypass-secret", "x"]).is_err());
        assert!(try_parse(&["status", "--ca-file", "a.pem", "--insecure"]).is_err());
        assert!(try_parse(&["maintenance", "on", "--ttl", "soon"]).is_err());
    }

    #[test]
    fn the_config_flag_works_after_a_subcommand_too() {
        let cli = parse(&[
            "token",
            "list",
            "--config",
            "other.toml",
            "--database",
            "x.db",
        ]);
        assert_eq!(cli.config, PathBuf::from("other.toml"));
        assert_eq!(cli.database, Some(PathBuf::from("x.db")));
    }

    #[test]
    fn check_database_may_name_a_path_or_not() {
        let check = |args: &[&str]| {
            let Some(Command::Config {
                action: ConfigAction::Validate(validate),
            }) = parse(args).command
            else {
                panic!("config validate")
            };
            validate.check_database
        };
        assert_eq!(check(&["config", "validate"]), None);
        assert_eq!(
            check(&["config", "validate", "--check-database"]),
            Some(None)
        );
        assert_eq!(
            check(&["config", "validate", "--check-database", "x.db"]),
            Some(Some(PathBuf::from("x.db")))
        );
    }

    #[test]
    fn admin_auth_overrides_the_config_and_rejects_other_values() {
        let mut config = AppConfig::default();
        parse(&["--admin-auth", "off"]).apply(&mut config);
        assert_eq!(config.admin.auth, AdminAuth::Off);
        parse(&["--admin-auth", "required"]).apply(&mut config);
        assert_eq!(config.admin.auth, AdminAuth::Required);
        assert!(Cli::try_parse_from(["fast-proxy", "--admin-auth", "maybe"]).is_err());
    }

    #[test]
    fn admin_tls_flag_turns_admin_tls_on() {
        let mut config = AppConfig::default();
        assert!(!config.admin.tls.enabled);
        parse(&["--admin-tls"]).apply(&mut config);
        assert!(config.admin.tls.enabled);
        let mut untouched = AppConfig::default();
        parse(&[]).apply(&mut untouched);
        assert!(!untouched.admin.tls.enabled);
    }

    #[test]
    fn no_flags_leave_the_config_untouched() {
        let mut config = AppConfig::default();
        parse(&[]).apply(&mut config);
        assert_eq!(config.admin.listen.port(), 9000);
        assert_eq!(config.logging.level, "info");
        assert!(config.data_plane.database.is_none());
    }

    #[test]
    fn flags_override_the_matching_values() {
        let mut config = AppConfig::default();
        parse(&[
            "--admin-listen",
            "127.0.0.1:9100",
            "--database",
            "/tmp/x.db",
            "--state-dir",
            "/tmp/state",
            "--log-level",
            "debug",
            "--access-log",
            "/tmp/a.log",
        ])
        .apply(&mut config);
        assert_eq!(config.admin.listen.port(), 9100);
        assert_eq!(
            config.data_plane.database_path(),
            PathBuf::from("/tmp/x.db")
        );
        assert_eq!(config.data_plane.state_dir, PathBuf::from("/tmp/state"));
        assert_eq!(config.logging.level, "debug");
        assert_eq!(config.logging.access_log, Some(PathBuf::from("/tmp/a.log")));
    }

    #[test]
    fn no_access_log_wins_over_the_file_value() {
        let mut config = AppConfig::default();
        config.logging.access_log = Some(PathBuf::from("access.log"));
        parse(&["--no-access-log"]).apply(&mut config);
        assert!(config.logging.access_log.is_none());
    }

    #[test]
    fn tls_flags_override_the_file_in_both_directions() {
        let mut config = AppConfig::default();
        parse(&["--tls", "--https-listen", "127.0.0.1:8444"]).apply(&mut config);
        assert!(config.data_plane.tls.enabled);
        assert_eq!(config.data_plane.tls.listen.port(), 8444);

        config.data_plane.tls.enabled = true;
        parse(&["--no-tls"]).apply(&mut config);
        assert!(!config.data_plane.tls.enabled);

        let mut untouched = AppConfig::default();
        parse(&[]).apply(&mut untouched);
        assert!(!untouched.data_plane.tls.enabled, "off by default");
    }

    #[test]
    fn tls_and_no_tls_conflict() {
        assert!(Cli::try_parse_from(["fast-proxy", "--tls", "--no-tls"]).is_err());
    }

    #[test]
    fn the_old_host_and_port_flags_are_gone() {
        assert!(Cli::try_parse_from(["fast-proxy", "--port", "1"]).is_err());
        assert!(Cli::try_parse_from(["fast-proxy", "--host", "x"]).is_err());
    }
}
