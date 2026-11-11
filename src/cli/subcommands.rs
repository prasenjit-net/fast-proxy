//! The subcommands: operations against a running proxy, `config validate`, and
//! offline token management (ADR 0019). With no subcommand the binary runs the
//! server, as it always did.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Turn maintenance mode on or off, or show what is on
    Maintenance {
        #[command(subcommand)]
        action: MaintenanceAction,
    },
    /// Make a running proxy re-read its database now
    Reload(RemoteArgs),
    /// Show what a running proxy is doing
    Status(RemoteArgs),
    /// Show recent audit entries of a running proxy
    Audit(AuditArgs),
    /// Check configuration without starting anything
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage admin tokens directly on the database file (recovery)
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

/// How to reach a running proxy's admin plane.
#[derive(Args, Debug, Clone, Default)]
pub struct RemoteArgs {
    /// The admin plane's URL (default: derived from the config file's [admin] section)
    #[arg(long)]
    pub url: Option<String>,
    /// The admin token. Prefer FAST_PROXY_TOKEN or --token-file: a flag shows in the process list
    #[arg(long)]
    pub token: Option<String>,
    /// A file holding the admin token
    #[arg(long)]
    pub token_file: Option<PathBuf>,
    /// A PEM file of the CA that issued the admin certificate
    #[arg(long, conflicts_with = "insecure")]
    pub ca_file: Option<PathBuf>,
    /// Do not verify the admin certificate (a self-signed one)
    #[arg(long)]
    pub insecure: bool,
    /// Print the proxy's JSON answer instead of text
    #[arg(long)]
    pub json: bool,
}

/// What a maintenance command covers: the whole proxy, a subsystem, or a route.
#[derive(Args, Debug, Clone, Default)]
pub struct ScopeArgs {
    /// A subsystem, by name or id (omit for the whole proxy)
    #[arg(long)]
    pub subsystem: Option<String>,
    /// A route of that subsystem, by prefix such as /api
    #[arg(long, requires = "subsystem")]
    pub route: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum MaintenanceAction {
    /// Show every scope that is on
    Status(RemoteArgs),
    /// Turn maintenance on (replacing earlier settings of that scope)
    On(MaintenanceOn),
    /// Turn maintenance off
    Off(MaintenanceOff),
}

#[derive(Args, Debug)]
pub struct MaintenanceOn {
    #[command(flatten)]
    pub remote: RemoteArgs,
    #[command(flatten)]
    pub scope: ScopeArgs,
    /// How long it lasts, such as 30m or 2h (default: until turned off)
    #[arg(long, value_parser = humantime::parse_duration)]
    pub ttl: Option<Duration>,
    /// Shown to visitors
    #[arg(long, default_value = "")]
    pub message: String,
    /// The Retry-After seconds (default: derived from the TTL, else 120)
    #[arg(long)]
    pub retry_after: Option<u32>,
    /// A source network that passes, such as 10.0.0.0/8 (repeatable)
    #[arg(long = "bypass-cidr")]
    pub bypass_cidrs: Vec<String>,
    /// A request header whose secret value lets a request pass
    #[arg(long)]
    pub bypass_header: Option<String>,
    /// The secret for --bypass-header (generated and shown once if omitted)
    #[arg(long, requires = "bypass_header")]
    pub bypass_secret: Option<String>,
}

#[derive(Args, Debug)]
pub struct MaintenanceOff {
    #[command(flatten)]
    pub remote: RemoteArgs,
    #[command(flatten)]
    pub scope: ScopeArgs,
}

#[derive(Args, Debug)]
pub struct AuditArgs {
    #[command(flatten)]
    pub remote: RemoteArgs,
    /// How many entries to show
    #[arg(long, default_value_t = 20)]
    pub limit: u32,
    /// Only this actor (a token name)
    #[arg(long)]
    pub actor: Option<String>,
    /// Only this action, such as maintenance.set
    #[arg(long)]
    pub action: Option<String>,
    /// Only `ok`, `denied` or `failed`
    #[arg(long)]
    pub outcome: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Parse and check the configuration file; with --check-database also compile the stored configuration
    Validate(ValidateArgs),
}

#[derive(Args, Debug)]
pub struct ValidateArgs {
    /// Also open the database read-only and compile the configuration stored in it
    /// (the configured database, or PATH)
    #[arg(long = "check-database", value_name = "PATH", num_args = 0..=1)]
    pub check_database: Option<Option<PathBuf>>,
}

#[derive(Subcommand, Debug)]
pub enum TokenAction {
    /// Create a token and print it once
    Create(TokenCreate),
    /// List tokens (never their secrets)
    List,
    /// Revoke a token by id or name
    Revoke {
        /// The token's id or name
        token: String,
    },
}

#[derive(Args, Debug)]
pub struct TokenCreate {
    /// A unique name, shown in the audit log
    #[arg(long)]
    pub name: String,
    /// `admin` or `viewer`
    #[arg(long, default_value = "admin")]
    pub role: String,
    /// Expire after this many days (default: never)
    #[arg(long)]
    pub expires_days: Option<u32>,
}
