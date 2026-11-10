//! Command-line flags and how they override the config file.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use crate::config::AppConfig;

/// Fast, realtime-configurable ingress server and reverse proxy with an embedded admin UI.
///
/// Configuration is read from a TOML file; every flag below overrides
/// the corresponding value from that file.
#[derive(Parser, Debug)]
#[command(name = "fast-proxy", version, about)]
pub struct Cli {
    /// Path to the TOML configuration file
    #[arg(short, long, default_value = "config.toml")]
    pub config: PathBuf,

    /// Override [admin].listen (for example 127.0.0.1:9000)
    #[arg(long)]
    pub admin_listen: Option<SocketAddr>,

    /// Override [data_plane].database
    #[arg(long)]
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
