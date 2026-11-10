# ADR 0019: the CLI and reload

Status: proposed (awaiting owner confirmation). Plan reference: M5 H15, H16, H17.

## Decision

- **No subcommand runs the server exactly as today.** New subcommands:
  `maintenance status|on|off`, `reload`, `status` and `audit` talk to a running
  proxy through the admin API (`--url`, a token from `--token`,
  `FAST_PROXY_TOKEN` or `--token-file`, `--ca-file` or `--insecure` for admin TLS).
  `config validate` parses and validates the configuration file and, with
  `--database`, compiles the stored configuration read-only, creating nothing.
  `token create|list|revoke` work offline on the database file and write audit
  entries with the actor `cli`.
- **Exit codes:** 0 success, 1 the proxy refused or validation failed, 2 usage or
  connection errors.
- **No new HTTP dependency:** the client reuses hyper and hyper-rustls.
- **`reload`** (and `POST /api/reload`) re-reads the database now and recompiles,
  the sync poll's reload without the version check, and returns the live version.
  Subsystems, routes and upstreams are live already; this is for a database edited
  by hand, `config_poll_interval = "0s"`, or a restore. Process settings still need
  a restart.
- **Crates:** a new `fast-proxy-auth` (token format, hashing, constant-time
  verification, roles, throttle, tickets) with no HTTP and no database, depending
  on `model` only. `model` gains the maintenance types and `Role`/`Action`;
  `store` the tables as opaque rows (it never hashes or verifies); `routing` the
  compiled maintenance; `config-service` the writes, actor context, audit
  transaction, sweeper and token cache; `dataplane` the check and response;
  `admin` the middleware and endpoints; the binary the CLI. Dependencies point one
  way.

## Consequences

- The token file permission is checked on Unix.
- A token passed on the command line is visible in the process list; the
  environment variable and the file are the recommended sources.
