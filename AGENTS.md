# AGENTS.md

Guidance for coding agents working in this repository.

## Project Overview

`fast-proxy` is a fast, realtime-configurable ingress server and reverse proxy (see `docs/ROADMAP.md`). Milestones 1 and 2 are complete: a working HTTP reverse proxy with host-based subsystems, prefix routing, live configuration stored in SQLite with history and rollback, WebSocket passthrough, graceful draining, Prometheus metrics, and an admin UI (M1); upstream groups with several endpoints, five load-balancing algorithms, active and passive health checking, safe retries, and drain/disable of endpoints (M2). Milestone 3 is complete too: TLS termination with per-subsystem certificates stored encrypted in SQLite and managed live, HTTP/2 to clients, redirect and HSTS, TLS to upstreams, and several instances sharing one database. Milestone 5 is complete as well: the admin plane needs a bearer token (two roles), every change is audited with its actor, maintenance mode closes the whole proxy, a subsystem or one route (503, TTL, bypass), the admin plane can serve TLS, and a CLI does the common operations. ACME is the next milestone (M4 was skipped).

- Rust workspace: a data plane on hyper, an admin plane on Axum, SQLite via rusqlite, Tokio, Clap, Serde, Tracing, and `rust-embed`.
- React frontend under `ui/`, built with Vite, TypeScript, Tailwind CSS v4, TanStack Router, and TanStack Query.
- The frontend build output is embedded into the Rust binary from `ui/dist`.
- Design and plans are in `docs/` (`ROADMAP.md`, `M1-PLAN.md`, `adr/`).

## Repository Layout

Cargo workspace; the root package is the `fast-proxy` binary.

- `src/main.rs` wires CLI parsing, configuration, and logging; `src/startup.rs`
  assembles the store, config service, data plane, and admin plane.
- `src/config/` defines the TOML configuration (`[data_plane]`, `[admin]`, `[logging]`, `[ui]`) and its validation; `src/cli.rs` defines the CLI overrides.
- `crates/fast-proxy-admin/` is the Axum admin plane: `router.rs` and `route_table.rs`
  (every route is registered with an `Access`: `Open`, `Needs(Action)` or `Ticket(Action)`),
  `guard.rs` (throttle, bearer authentication, role check), `access_middleware.rs` and
  `refusal_audit.rs` (the access log and the audit of refusals), `host_allow_list.rs`,
  `security_headers.rs`, `tls_server.rs` (the admin listener over TLS), `api/` (one
  file per resource: subsystems, hosts, upstreams, routes, certificates, tokens, maintenance,
  audit, session, reload), `ws.rs`, `events.rs`, `error.rs` (shared API error
  envelope), `static_assets.rs` (embedded SPA from `ui/dist`), `state.rs`.
- `crates/fast-proxy-auth/` is token and secret handling with no HTTP and no database:
  generation, SHA-256 hashing, constant-time comparison (`subtle`), roles, the failure
  throttle, the WebSocket ticket store. `crates/fast-proxy-model/` holds the vocabulary
  (`Role`, `Action`, `SecretHash`, `Cidr`, the maintenance types).
- `crates/fast-proxy-telemetry/` holds `Metrics` (Prometheus registry, the single
  source for `/metrics` and the live traffic stats), the `AccessLog`
  (non-blocking; `flush()` at shutdown) and the `EndpointEvents` bus (endpoint
  health and ejection changes). Metric labels must come only from config
  values (names, endpoint addresses), never raw hosts or paths; series of
  removed endpoints and groups are deleted (`retain_upstream_series`).
- `crates/fast-proxy-model/` holds validated domain types (`HostPattern`,
  `RoutePrefix`, `EndpointAddr`, ...) and the `Config` tree; no I/O.
- `crates/fast-proxy-balancing/` is pure: `EndpointRuntime` (atomics: in-flight,
  health, ejection, latency), `RuntimeRegistry`, the selection algorithms and
  `Balancer` (eligibility and the failure policy), the passive-health state
  machine (`record_outcome`), `RetryBudget`. State machines take `now` as a
  parameter; `monotonic_ms()` is the one process clock callers use.
- `crates/fast-proxy-health/` runs the active probes: `HealthEngine` keeps one
  prober task per endpoint of groups with checks on, reconciled with every
  config change, plus the status gauges sampler.
- `crates/fast-proxy-routing/` compiles a `Config` (plus the maintenance scopes, through
  `compile_all`) into an immutable `Snapshot`
  (host and route matchers, and each group as a `CompiledUpstream` holding its
  `Balancer` and endpoint runtime) shared through `SnapshotHolder`; also owns
  `build_upstream_uri` (path rewriting), used by both the data plane and the
  admin resolver so they can never disagree. Maintenance is decided by one pure function,
  `CompiledMaintenance::check`, used by the data plane and the resolver alike. No I/O.
- `crates/fast-proxy-store/` is SQLite persistence: `Database` (sync core, all
  writes via `Database::write`), `Store` (async handle on a dedicated thread),
  `repository/` (one file per resource), `revision/`. The data plane must never
  use it.
- `crates/fast-proxy-config-service/` is the only writer: `ConfigService::apply`
  validates (compiles) a change, persists it with its revision and its audit row in one
  transaction, swaps the live `Snapshot`, and broadcasts `ConfigChanged`. All admin writes
  must go through it, each with a `ChangeContext` (who, from where, which request) and an
  `Audited` description. It also owns tokens (`tokens.rs`), maintenance (`maintenance.rs`
  and its sweeper), audit retention and `record_audit` (refusals, which never make a
  revision).
- `crates/fast-proxy-dataplane/` is the hyper proxy: `listener` (bind),
  `server` (accept loop), `pipeline` (host -> route -> `Forward`), `responses`
  (fixed 404/503/502/504), `selection` (pick an endpoint per request),
  `upstream_client` (pooled forwarding, the retry loop, passive outcomes),
  `retry_policy` (pure retry rules), `replay_body` (a request body shared across
  attempts), `pending_request` (reports a request cancelled by its client as
  499), `protocol_switch` (upgrades), `headers` (hop-by-hop, `X-Forwarded-*`), `connections` (open
  connection tracking for draining). `DataPlane::stopped(grace)` drains. It reads
  snapshots only and never depends on the store.
- `crates/fast-proxy-tls/` is everything about certificates and keys, with no HTTP
  knowledge: PEM parsing and validation, the `Vault` (AES-256-GCM under the master
  key), the self-signed generator, `CertCache` (parsed keys, unchanged entries
  reused across rebuilds), rustls server and client configurations.
  `crates/fast-proxy-test-pki/` is test support only (an in-memory CA, TLS test
  clients); never use it outside `dev-dependencies` and tests.
- `src/cli/` and `src/commands/` are the subcommands (`maintenance`, `reload`, `status`,
  `audit`, `config validate`, and the offline `token` commands); exit code 0 success, 1 the
  proxy refused or a check failed, 2 usage or connection errors.
- `docs/adr/` records design decisions; `docs/getting-started.md` is the manual
  curl walkthrough (keep it working when the API changes).
- `tests/` holds end-to-end tests (the real binary, and in-process stacks with real TLS clients in `tests/common/tls_stack.rs`); shared helpers (spawning
  a configured proxy, backends, a tiny HTTP client) are in `tests/common/mod.rs`,
  with `flaky.rs` (a backend that can be killed, revived, slowed and made to
  fail), `groups.rs` (admin helpers for groups and endpoints) and `load.rs` (a
  keep-alive load client). `upstream_groups.rs` is the M2 exit criteria; `operations.rs`,
  `maintenance.rs`, `maintenance_under_load.rs`, `secret_hygiene.rs`, `cluster_operations.rs`,
  `admin_auth.rs`, `admin_tls.rs` and `cli.rs` are M5's (`common/console.rs` starts the
  binary with authentication on and reads the bootstrap token from its console).
  Tests that deliver real signals are `#[cfg(unix)]`.
- `docs/benchmarks.md` records the performance baseline and how to reproduce it
  (measured with `oha` against the `bench_backend` example in the data plane crate).
- `ui/src/lib/` holds the frontend API client (`http.ts` core and `ApiError`,
  `types.ts`, `api.ts` one function per endpoint), `validation.ts` (client-side
  checks that use the server's camelCase field names), and `navigation.ts`.
- `ui/src/context/` contains Theme, Toast, Config, and WebSocket live data providers.
- `ui/src/components/` contains reusable UI components; `forms/` has one form per
  resource and `subsystem/` the detail page's sections.
- `ui/src/pages/` contains route pages.
- `ui/src/styles/index.css` contains Tailwind entrypoint and theme tokens.

## Architecture rules

These invariants hold across the codebase; do not break them without an ADR in `docs/adr/`.

- **The data plane never touches the database.** It only reads the compiled
  `Snapshot` through `SnapshotHolder`. `fast-proxy-dataplane` must not depend on
  `fast-proxy-store`.
- **All configuration writes go through `ConfigService::apply`** (mutate in a
  transaction, compile the would-be configuration as the validation, commit with
  a revision, swap the snapshot). Never write to the store or swap a snapshot
  from anywhere else.
- **`model` and `routing` are pure** (no I/O). Cross-table rules are checked by
  `compile`; field-level rules by the `parse` constructors in `model`.
- **A request loads the snapshot once** and uses it to the end.
- **Configuration is split:** `config.toml` is process settings only (restart to
  change); subsystems, hosts, upstreams and routes are in the database.
- **Metric labels come only from config names** (subsystem name, route prefix,
  `_unmatched`/`_none`), never raw hosts or paths.
- **Endpoint runtime state lives in `fast-proxy-balancing`** and is carried across
  snapshot swaps by endpoint id and address (`compile(.., &registry)`); never
  keep it in config or in a global map. Endpoint ids are stable and rollback
  preserves them.
- **Retries are conservative:** a connect failure may be retried for any method,
  everything else only for idempotent ones; never after a body byte was read
  (`BodyOwner::is_replayable`), never for upgrades, always within the group's
  retry budget and away from the endpoint that just failed.
- **Draining and disabled endpoints are never selected**, not even in panic mode.
- **The URL rewrite lives in `fast-proxy-routing`** (`build_upstream_uri`) so the
  proxy and the admin resolver cannot disagree.
- **API errors never leak paths or storage detail**; the detail goes to the log.
- **Every admin route declares an `Access`** when it is registered (`RouteTable`); a test
  fails for a route that does not, and for one that skips the guard. Only `/api/health` is
  open. A new route needs an `Action` (`Read`, `Write`, `ManageTokens`, ...).
- **Tokens, audit entries and maintenance scopes are not `Config`.** A revision never holds
  them and a rollback never changes them (it must not revoke today's tokens or switch
  maintenance on or off). A change to any of them is still a revision (summary only), so
  other instances pick it up through the sync poll.
- **Secrets are stored only as SHA-256 hashes, shown once, and compared in constant time**
  (`fast-proxy-auth`). A token, a bypass secret, a WebSocket ticket or a key appears in no
  log line, metric, audit entry, revision, event or API response (`tests/secret_hygiene.rs`
  checks every one against the real binary). Every type that holds one has a redacted `Debug`.
- **The audit row is written in the same transaction as the change.** Refusals (401, 403,
  421, rejected writes) are written by the access middleware through `record_audit`;
  entries for callers that are not authenticated are capped per minute for the process.
- **Maintenance never reaches an upstream and never touches endpoint state.** The check
  runs after the host matched and before an endpoint is selected; bypass is by effective
  client address (`X-Forwarded-For` only from a trusted proxy) or a secret header, and the
  bypass header is removed from the request before it is forwarded. Health checks keep
  running; the admin plane is never in maintenance.
- **The admin plane needs authentication.** `auth = "off"` is accepted only on a loopback
  listener; a non-loopback listener needs TLS and authentication or it refuses to start. A
  request that carries a private key needs an admin token over a protected channel
  (loopback, or admin TLS).
- **Certificate material and keys never appear in `Config`, revisions, logs, errors,
  metrics, events or API responses.** Private keys are write-only: sealed by the
  `Vault` before they reach the store, opened only into the `CertCache`. Every type
  that holds one has a redacted `Debug`. Requests that carry a key need an admin
  token over a protected channel.
- **Certificate and HTTPS writes go through `ConfigService` like every other write**
  (seal, store, reconcile development certificates, veto by compiling with the
  would-be certificates, commit, swap). There is no other reload path.
- **The `store` crate never decrypts** and has no cryptography dependency (a test
  checks the manifest); it sees certificate keys only as ciphertext.
- **Certificates travel in the routing snapshot**, so a certificate or HTTPS change
  is one atomic swap; a handshake chooses its certificate once from one snapshot
  and the 421 rule compares ids from that choice.
- **A self-signed certificate is generated only for a subsystem that enables HTTPS**
  (named `dev-<subsystem>`), never for a name nobody configured; an uploaded
  certificate is never replaced automatically.
- **The redirect `Location` is built only from a host that matched a configured
  pattern and passed `dns_host`**, plus the original path and query; HSTS is sent
  only on HTTPS responses.
- **Sharing one database between instances** relies on the revision version as the
  generation (`ConfigService::sync_once`); everything local to an instance (health,
  in-flight counts, session tickets) stays out of the database.

## Development Rules

These rules are set by the project owner and apply to every session.

### Workflow

- **Never commit or push unless the owner explicitly says so.** When a task is
  done, stop and report. The owner does a manual check first, then asks for the
  commit and push.
- Plans and roadmap live in `docs/` (`ROADMAP.md`, `M1-PLAN.md`, `M2-PLAN.md`, `M3-PLAN.md`, `M5-PLAN.md`). Keep them
  current: tick tasks as they land and record decisions. Start a milestone by
  writing its `docs/Mn-PLAN.md`; record new design decisions as an ADR in
  `docs/adr/` and in the ROADMAP decision log.

### Design and structure

- Build with a **modular design**. Prefer many small, well-named source files
  over large ones; one clear responsibility per file and per type.
- **Subcrates are encouraged.** Put separable logic (for example storage,
  routing/snapshot, the proxy data plane, the admin API) in its own crate in a
  Cargo workspace, with a narrow public API and minimal dependencies between
  crates. Dependencies point one way; no cycles.
- **Naming matters.** Name modules, structs, enums, traits, functions, and
  variables for what they mean. Avoid vague names (`data`, `manager`, `util`,
  `handle_it`).
- Write **clean code**: small functions, early returns, no dead code, no
  commented-out code, no duplicated logic, errors handled explicitly (no
  `unwrap`/`expect` in non-test code unless an invariant is documented).

### Documentation in code

- Add a **light doc comment** (`///`, and `//!` for modules and crates) to every
  public item, and to private items where intent is not obvious. Say what and
  why in a sentence or two; do not restate the code.
- Use inline comments only where the reason for something is not evident.

### Testing

- **Always write tests** for new and changed behavior, in the same change.
  Unit tests live next to the code (`#[cfg(test)]`); cross-crate and end-to-end
  scenarios go in `tests/`. Frontend changes get Vitest tests.
- Keep **overall test coverage above 60%** (aim higher for core logic such as
  matchers, validation, and the config pipeline). Measure it, for example with
  `cargo llvm-cov`, and report the number when finishing a task. Do not lower
  coverage to ship a change.
- Run the checks listed under Testing Expectations before reporting a task done.
- **Tests must pass on Linux, macOS and Windows** (CI runs all three). A refused
  connection takes about a second to be reported on Windows, so tests that wait
  on one must wait for the event with a generous limit, never a fixed short
  sleep. Do not
  shell out to Unix commands (`kill`, `sh`) in tests without `#[cfg(unix)]`. In
  tests that talk raw TCP, send each request in one write and set `TCP_NODELAY`:
  tiny split writes stall for tens of milliseconds on Linux (Nagle plus delayed
  ACK) and hide as slow or flaky tests. Pick free ports with retry, since a
  parallel test can take a port between picking and binding.
- When a test passes on the first try, check it can fail: temporarily break the
  behavior it guards and confirm it goes red.

### Compatibility and migration

- The project is **not deployed anywhere yet**. Do not add backward
  compatibility, data migrations, deprecation shims, or upgrade paths for
  config files, DB schemas, APIs, or behavior. Change things directly; for
  example, edit the initial DB schema in place instead of adding a migration.
  Revisit this rule only when the owner says the project has been deployed.

## Common Commands

Backend:

```sh
cargo test
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo run
```

Frontend:

```sh
cd ui && npm test
cd ui && npm run build
cd ui && npm run dev
```

Full build/run:

```sh
make build
make run
```

Coverage and benchmarks:

```sh
cargo llvm-cov --workspace --summary-only     # needs cargo-llvm-cov; report the number
cargo test --test exit_criteria               # end-to-end tests against the real binary
cargo test --test reload_under_load -- --nocapture
# performance baseline and how to reproduce it: docs/benchmarks.md
```

Development servers:

- Backend: `cargo run`; admin plane on `127.0.0.1:9000`, data plane on `0.0.0.0:8000`.
- Frontend: `cd ui && npm run dev` on `localhost:5173`, proxying `/api` and `/ws` to the backend.

## Implementation Notes

- Keep API responses using the existing `AppError` / JSON error envelope pattern.
- Add new REST handlers under `crates/fast-proxy-admin/src/api/`, register routes in `crates/fast-proxy-admin/src/router.rs`, and expose typed frontend calls from `ui/src/lib/api.ts`.
- Add new WebSocket event types in `crates/fast-proxy-admin/src/events.rs` and handle them in `ui/src/context/LiveContext.tsx`.
- Add new pages under `ui/src/pages/`, then update routing and navigation in the existing frontend structure.
- Keep UI styling aligned with the tokens in `ui/src/styles/index.css`; avoid hard-coded one-off colors when a token exists.
- Prefer existing component patterns in `ui/src/components/` before adding new abstractions.
- UI forms: validate in `lib/validation.ts`, show server `fields` inline via `useServerErrors`, and use toasts for success. Page tests use `test/renderPage.tsx` and `test/mockApi.ts`.
- Do not commit generated artifacts such as `target/`, `ui/dist/`, or `ui/node_modules/` unless explicitly requested.

## Testing Expectations

- For backend changes, run at least `cargo test`; also run `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` when practical.
- For frontend changes, run `cd ui && npm test`; run `cd ui && npm run build` for type-checking and production bundle verification.
- For changes spanning backend and frontend contracts, test both sides.
- Report test coverage (see Development Rules) when finishing a task.

## Configuration

- Runtime config defaults to `config.toml`.
- CLI flags can override config values: `--admin-listen`, `--database`, `--state-dir`, `--log-level`, `--access-log`, `--no-access-log`, and `--config`.
- The config file holds process-level settings only; subsystems, routes, and upstreams live in the database.
- UI boot configuration is served from `GET /api/config` based on the `[ui]` section of `config.toml`.

