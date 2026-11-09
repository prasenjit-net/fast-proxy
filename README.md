# fast-proxy

[![CI](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml)

A fast, **realtime-configurable ingress server and reverse proxy** written in
Rust, shipped as a single binary with an embedded admin UI.

You run one process, open the admin UI, and describe what to serve: each host
name is a **subsystem** with its own upstreams and routes. Changes apply to live
traffic immediately, without a restart and without dropping a request, and every
change is recorded so it can be rolled back.

> **Status: milestone 1 is feature-complete** (release `v0.2.0` is being
> prepared). It is a working HTTP reverse proxy with live configuration. TLS,
> load balancing, health checks, ACME and maintenance mode are the next
> milestones; see the [roadmap](docs/ROADMAP.md). The admin plane has **no
> authentication yet** (see [Security notes](#security-notes)).

## What it does today

- **Host-based virtual hosting.** A request is matched to a subsystem by its
  `Host` header: exact names first, then the longest `*.wildcard` suffix.
- **Prefix routing.** Inside a subsystem the longest matching path prefix wins,
  on path-segment boundaries (`/api` matches `/api/x`, not `/apix`), with
  optional prefix stripping, an upstream path prefix, and per-route timeouts.
- **Live configuration.** Subsystems, hosts, upstreams and routes live in a local
  SQLite file and are edited through the admin UI or REST API. A change is
  validated, saved, and swapped in atomically; an invalid change is rejected and
  the old configuration keeps serving. It survives restarts.
- **History and rollback.** Every change is a numbered revision; any earlier
  state can be viewed and re-applied.
- **A streaming proxy.** Request and response bodies stream without buffering,
  upstream connections are pooled, WebSocket and other protocol upgrades pass
  through, and `X-Forwarded-*`, hop-by-hop headers and request ids are handled
  (client-supplied forwarding headers are not trusted unless the peer is listed
  in `trusted_proxies`).
- **Graceful shutdown.** Ctrl-C stops accepting, closes idle connections, and
  lets in-flight requests and open WebSockets finish for up to
  `data_plane.shutdown_grace`.
- **Observability.** Prometheus metrics at `/metrics`, one access-log line per
  request, live traffic stats and a dashboard, and a resolver that shows exactly
  where a given host and path would be sent.

## Quick start

Prerequisites: Rust (stable) and the Node version in `ui/.nvmrc`.

```sh
# 1. Build the frontend (it is embedded into the binary by the Rust build)
cd ui && npm install && npm run build && cd ..

# 2. Build and run the single binary
cargo run --release
# admin UI + API  -> http://127.0.0.1:9000
# the proxy       -> http://0.0.0.0:8000
```

Or just `make build` / `make run`. Then open the admin UI, create a subsystem,
add its host name, an upstream (your backend's `host:port`) and a route with
prefix `/`, and send a request with that `Host` header to port 8000.

**[docs/getting-started.md](docs/getting-started.md)** walks through the same
thing with `curl`, step by step: two subsystems, live edits, the resolver,
rollback, metrics, WebSockets, shutdown, and restart.

### Development (hot reload)

Run the two dev servers side by side:

```sh
cargo run                # terminal 1: backend (admin plane on :9000)
cd ui && npm run dev     # terminal 2: Vite on :5173, proxies /api and /ws
```

Open http://localhost:5173. In debug builds `rust-embed` reads `ui/dist` from
disk, so a production `npm run build` is picked up by a plain restart too.

## How it works

Two planes on two listeners:

```
                 config.toml                         SQLite file
                     |                                   ^
                     v                                   | (one writer thread)
  +----------------- main ------------------------------+ |
  |                                                      | |
  |  data plane (hyper)             admin plane (axum)   | |
  |  0.0.0.0:8000                   127.0.0.1:9000       | |
  |     |                           REST + WS + UI       | |
  |     v                           /metrics             | |
  |  load Snapshot (arc-swap) <--- swap -- ConfigService | |
  |     |                                (validate, save, compile, swap)
  |  host -> subsystem -> prefix route -> upstream       |
  |     |                                                |
  |  pooled client  --> your backends                    |
  +------------------------------------------------------+
```

- **The data plane** serves public traffic. Each request loads the current
  immutable routing *snapshot* once and uses it to the end, so a config change
  never affects a request already in flight. It never reads the database.
- **The admin plane** is the UI, REST API, WebSocket push and `/metrics`. It
  binds to loopback by default. All writes go through one `ConfigService`:
  run the change in a transaction, compile the would-be configuration (this is
  the validation), commit with a revision, swap the new snapshot, notify the UI.
- **Configuration is split on purpose.** `config.toml` holds how the process is
  hosted (listeners, state directory, admin address, logging) and needs a
  restart to change. Subsystems, hosts, upstreams and routes live in the
  database and change live. The file never contains routes; the database never
  contains listeners.

The design and the reasoning behind it are in [docs/ROADMAP.md](docs/ROADMAP.md)
and the decision records in [docs/adr/](docs/adr/).

## Capabilities and roadmap

| Capability | Status | Milestone |
|---|---|---|
| Subsystems by host name (exact and wildcard) | Done | M1 |
| URI prefix routing with strip/rewrite and timeouts | Done | M1 |
| Realtime configuration with history and rollback | Done | M1 |
| WebSocket and protocol-upgrade passthrough; graceful draining | Done | M1 |
| Prometheus metrics, access log, live traffic stats, admin UI | Done | M1 |
| Upstream groups with several endpoints, load balancing | Planned | M2 |
| Upstream health monitoring (active probes, outlier ejection) | Planned | M2 |
| TLS termination (rustls, SNI, hot reload, HTTP/2) | Planned | M3 |
| ACME automatic certificates | Planned | M4 |
| Maintenance mode, admin authentication, audit log | Planned | M5 |
| Request sampling of unusual requests | Planned | M6 |
| Rate and connection limits, hardening, 1.0 | Planned | M7 |

## CLI

```
fast-proxy [OPTIONS]

  -c, --config <CONFIG>             Path to the TOML configuration file [default: config.toml]
      --admin-listen <ADDR>         Override [admin].listen
      --database <PATH>             Override [data_plane].database
      --state-dir <PATH>            Override [data_plane].state_dir
      --log-level <LOG_LEVEL>       Override [logging].level (trace, debug, info, warn, error)
      --access-log <PATH>           Override [logging].access_log file path
      --no-access-log               Disable the access log file entirely
```

## Configuration (`config.toml`)

| Key | Default | Meaning |
|---|---|---|
| `data_plane.state_dir` | `./state` | Directory for the database and other state; created at startup |
| `data_plane.database` | `<state_dir>/fast-proxy.db` | Database file |
| `data_plane.trusted_proxies` | `[]` | CIDRs whose `X-Forwarded-*` headers are trusted and appended to |
| `data_plane.shutdown_grace` | `30s` | Time in-flight requests get to finish on shutdown |
| `data_plane.listener` | one `http` listener on `0.0.0.0:8000` | Array of tables with `name` and `listen`; names and addresses must be unique |
| `admin.listen` | `127.0.0.1:9000` | Admin plane address; must differ from every listener (no authentication yet, keep it on loopback) |
| `logging.level` | `info` | Tracing filter (full directives allowed, e.g. `info,access=off`) |
| `logging.access_log` | *(unset)* | Access-log file path; omit to disable the file |
| `ui.app_name` | `Fast Proxy` | Shown in the sidebar + browser title |
| `ui.tagline` | ... | Shown under the app name |
| `ui.default_theme` | `auto` | `light` \| `dark` \| `auto`; used until the visitor picks |
| `ui.repo_url` | *(unset)* | Sidebar "Repository" link |

If the config file is missing, built-in defaults are used (with a warning).
Unknown keys are rejected, so a typo (or a `tls` key, which arrives in M3)
fails at startup instead of being ignored. Every listener is bound before any
traffic is served, so a bad address or a port in use aborts startup with a clear
message.

## API

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | Liveness + version |
| GET | `/api/config` | UI bootstrap config (the `[ui]` table, camelCased) |
| GET | `/api/status` | Listeners, database path, subsystem counts, uptime, live config version |
| GET | `/api/traffic` | Latest live traffic sample: requests/s, 5xx error rate, connections, per subsystem |
| GET | `/metrics` | Prometheus text exposition (admin listener only) |
| GET, POST | `/api/subsystems` | List (host patterns, route counts) / create |
| GET, PUT, DELETE | `/api/subsystems/{id}` | Detail (hosts, upstreams, routes in match order) / replace / delete |
| POST | `/api/subsystems/{id}/hosts` | Add a host pattern (`pattern`) |
| DELETE | `/api/subsystems/{id}/hosts/{host_id}` | Remove a host pattern |
| GET, POST | `/api/subsystems/{id}/upstreams` | List / create (`name`, `addr`, `weight`) |
| PUT, DELETE | `/api/subsystems/{id}/upstreams/{upstream_id}` | Replace / delete (409 while a route uses it) |
| GET, POST | `/api/subsystems/{id}/routes` | List in match order / create |
| PUT, DELETE | `/api/subsystems/{id}/routes/{route_id}` | Replace / delete |
| GET | `/api/revisions?limit=&before=` | Revision history, newest first (paged) |
| GET | `/api/revisions/{version}` | The full configuration as of that revision |
| POST | `/api/revisions/{version}/rollback` | Re-apply a revision as a new one |
| POST | `/api/resolve` | Dry-run `{host, path}` against the live config: subsystem, route, exact upstream URL, or why nothing matched |
| GET | `/ws` | WebSocket: server-push events |

Config responses are `{ "data": ..., "version": N }`, where `version` is the config
revision the response reflects (for a write, the one it created); deletes return
`{ "version": N }`.

Errors always look like:

```json
{ "error": { "code": "NOT_FOUND", "message": "subsystem 42 not found", "status": 404 } }
```

Validation failures (400) add a `fields` array naming each bad input
(camelCase), and duplicates are `409`:

```json
{ "error": { "code": "VALIDATION_FAILED", "message": "validation failed", "status": 400,
             "fields": [{ "field": "prefix", "message": "must start with '/'" }] } }
```

WebSocket events are JSON discriminated by `type`:

```json
{ "type": "traffic",  "data": { "total": { "requestsPerSec": 12.5, "errorRate": 0.02, "...": "..." }, "subsystems": ["..."] } }
{ "type": "configChanged", "version": 7, "summary": "route created: /api in shop", "timestampMs": 0 }
{ "type": "activity", "kind": "config", "message": "route created: /api in shop", "timestampMs": 0 }
{ "type": "hello",    "message": "Connected to Fast Proxy v0.1.0", "timestampMs": 0 }
```

## Observability

- **Metrics** at `GET /metrics` on the admin listener: `fastproxy_requests_total`
  (by subsystem, route and status class), `fastproxy_request_duration_seconds`,
  `fastproxy_upstream_errors_total` (connect, timeout, other),
  `fastproxy_active_connections`, `fastproxy_active_requests`,
  `fastproxy_config_version` and `fastproxy_config_reloads_total`. Label values
  come only from your configuration; unmatched traffic is `_unmatched`/`_none`.
- **Access log**, one line per request on both planes, to the console (tracing
  target `access`) and optionally a file. Proxied requests record subsystem,
  route, upstream, request id, upstream and total time, and bytes sent. The file
  is written off the request path; the console line is the expensive one (see
  [docs/benchmarks.md](docs/benchmarks.md): about a quarter of throughput), so
  filter it with `--log-level "info,access=off"` when throughput matters.
- **Performance baseline**: [docs/benchmarks.md](docs/benchmarks.md) (about 25 us
  added per request at p50 on one connection) and how to reproduce it.

## Security notes

- **The admin plane has no authentication yet** (authentication arrives in M5).
  It binds to `127.0.0.1` by default and logs a warning if you bind it
  elsewhere. Anyone who can reach it can reconfigure the proxy, so do not
  expose it to a network you do not trust.
- Client-supplied `X-Forwarded-*` headers are overwritten unless the peer is in
  `data_plane.trusted_proxies`.
- Config errors never echo file paths or storage details to API clients; the
  detail goes to the log.

## Project structure

```
├── docs/                     ROADMAP.md · M1-PLAN.md · M2-PLAN.md · getting-started.md · benchmarks.md · adr/
├── config.toml               process settings (listeners, admin, logging) + UI
├── src/
│   ├── main.rs               CLI entry + logging
│   ├── cli.rs                flags and how they override the config file
│   ├── config/               TOML config model and validation
│   └── startup.rs            assembles store, config service, data plane, admin plane
├── tests/                    end-to-end tests that run the real binary (exit criteria, reload under load)
├── crates/
│   ├── fast-proxy-model/     validated domain types and the Config tree (no I/O)
│   ├── fast-proxy-routing/   Config -> Snapshot compiler, host and route matchers (no I/O)
│   ├── fast-proxy-store/     SQLite storage, transactional writes, revisions, rollback
│   ├── fast-proxy-config-service/  the only writer: validate, persist, swap snapshot
│   ├── fast-proxy-dataplane/ hyper listeners, request pipeline, upstream forwarding
│   ├── fast-proxy-telemetry/ Prometheus metrics and the access log
│   └── fast-proxy-admin/     Axum admin plane: api/ · ws · events · error · embedded SPA
└── ui/
    ├── public/favicon.svg    the gear-and-bolt mark
    └── src/
        ├── lib/              typed API client (http, types, api), validation, navigation
        ├── icons/            the icon pack (add icons here)
        ├── context/          Theme · Toast · Config · Live (WebSocket)
        ├── components/       shell, forms/, subsystem/ sections, dialogs, chart, feed
        ├── pages/            Dashboard · Subsystems · SubsystemDetail · Revisions · Resolver · Settings · 404
        └── styles/           index.css (Tailwind v4 entry + theme tokens)
```

Dependencies point one way: `model` <- `store`, `routing`; `routing` +
`telemetry` <- `dataplane`; `store` + `routing` + `telemetry` <-
`config-service`; those <- `admin`; everything <- the binary. The data plane
never depends on the store.

## Extending

**New endpoint** - add a handler under `crates/fast-proxy-admin/src/api/`
returning `AppResult<Json<T>>`, register it in
`crates/fast-proxy-admin/src/router.rs`, add a typed function in
`ui/src/lib/api.ts` (and its types in `ui/src/lib/types.ts`). Writes must go
through `ConfigService::apply`.

**New page** - create `ui/src/pages/X.tsx`, add a route in `ui/src/router.tsx`, a
nav item in `components/Sidebar.tsx`, and a title in `lib/navigation.ts`.

**New server-push event** - add a variant to `Event` in
`crates/fast-proxy-admin/src/events.rs`, broadcast it via `state.broadcast(...)`,
handle its `type` in `ui/src/context/LiveContext.tsx`.

**Theming** - the design tokens live at the top of `ui/src/styles/index.css`
(one block per mode, registered as Tailwind colors via `@theme inline`).
Change the accent or surfaces there; every `bg-surface` / `text-ink` /
`bg-accent` utility picks the new values up in both light and dark mode.

## Testing

```sh
cargo test                       # unit tests per crate + end-to-end tests in tests/
cd ui && npm test                # frontend: Vitest + Testing Library
cargo llvm-cov --workspace --summary-only   # coverage (needs cargo-llvm-cov)
```

- `tests/exit_criteria.rs` runs the real binary: start from an empty database,
  configure it over the admin API, route traffic, edit live, roll back, restart,
  shut down gracefully (the signal tests run on Unix only).
- `tests/reload_under_load.rs` hammers the proxy with keep-alive clients while
  hundreds of config writes land, and checks that no request fails and no
  request is ever routed with a stale configuration.
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
  run in CI, which also tests on Linux, macOS and Windows.

## CI & releases

- **CI** (`.github/workflows/ci.yml`) runs on every push to `main` and every
  pull request: frontend test + build, `cargo fmt`/`clippy`, and `cargo test` +
  `cargo build --release` on Linux, macOS, and Windows.
- **Prepare Release** (`.github/workflows/release.yml`) is manual
  (Actions -> Prepare Release -> Run workflow, from `main`) and takes a `bump`
  input of `patch`, `minor`, or `major`. It updates `Cargo.toml`, `Cargo.lock`,
  `ui/package.json`, and `ui/package-lock.json`, then opens a `release/vX.Y.Z`
  pull request labeled `release`. The workflow fails before making changes if
  the release token is missing, the release branch already exists, the tag
  already exists, or a release PR for that version is already open.
- **Publish Release** (`.github/workflows/publish-release.yml`) runs on pushes
  to `main`, but publishes only when the pushed commit is the merged commit of a
  PR targeting `main` from a `release/` branch with the `release` label. It
  validates that all version files agree, builds binaries from the exact merge
  SHA, creates an annotated `vX.Y.Z` tag for that SHA, and creates or updates
  the GitHub Release. Reruns are safe when the existing tag already points to
  the same SHA; a tag pointing anywhere else fails the workflow. No release
  workflow commits directly to protected `main`.

### Release bot token

Create a repository secret named `RELEASE_BOT_TOKEN` for the Prepare Release
workflow. Use either a fine-grained personal access token or a GitHub App token
scoped only to this repository, with these repository permissions:

- Contents: read/write
- Pull requests: read/write
- Issues: read/write

The token is used only to push the `release/vX.Y.Z` branch, create the release
PR, and apply the `release` label.

## License

[MIT](LICENSE)
