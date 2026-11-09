# fast-proxy

[![CI](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml)

A fast, **realtime-configurable ingress server and reverse proxy** written in
Rust, shipped as a single binary with an embedded admin UI.

> **Status: early development.** The repository currently contains the
> single-binary scaffold (Axum admin API, WebSocket push, embedded React UI,
> TOML config, access log, CI/release pipeline). The proxy data plane is not
> built yet. See the [roadmap](docs/ROADMAP.md) for the plan and milestones.

## Planned capabilities

| Capability | Summary | Milestone |
|---|---|---|
| Subsystems by host name | Each host name is an independent, separately configurable subsystem, selected by `Host` (exact and wildcard) | M1 |
| URI prefix routing | Multiple routes per subsystem, longest-prefix match, optional strip/rewrite | M1 |
| Realtime configuration | Data plane (listeners, TLS, ACME) from the config file; subsystems and routes from a local DB, edited live in the admin UI | M1 |
| Upstream groups + load balancing | Round robin, weighted, least-connections, power-of-two-choices, consistent hash | M2 |
| Upstream health monitoring | Active probes plus passive outlier ejection | M2 |
| TLS termination | rustls, SNI cert selection, hot cert reload, HTTP/2 | M3 |
| ACME auto certificates | Automatic issue and renewal (HTTP-01 / TLS-ALPN-01) | M4 |
| Maintenance mode | Temporary, auto-expiring 503 per host or route, with bypass rules | M5 |
| Diagnostics and monitoring | Prometheus metrics, structured logs, live admin UI | M1-M5 |
| Request sampling | Capture unusual requests (errors, slow, anomalous) with redaction | M6 |

## Current scaffold features

These come from the template this project started from and will be reworked
or removed as the proxy lands (see the roadmap, M0).

- **Admin REST + WebSocket backend** - Axum with a typed error framework
  (`AppError` -> consistent JSON error envelope on every endpoint)
- **Embedded SPA** - `ui/dist` is compiled into the binary with `rust-embed`;
  deep links work via the SPA fallback, fingerprinted assets get immutable
  cache headers, unknown `/api/*` paths still return JSON 404s
- **Light / dark / auto theme** - Tailwind CSS v4 tokens, applied before first
  paint, visitor override persisted in `localStorage`
- **Dashboard UI** - stat tiles, a live chart, activity feed
- **UI configured by the backend** - the SPA boots from `GET /api/config`,
  which serves the `[ui]` section of `config.toml`
- **TOML configuration + CLI overrides** - every value in `config.toml` can be
  overridden with a flag (`--port`, `--host`, `--log-level`, `--access-log`,
  `--no-access-log`, `--config <path>`)
- **Access log** - per-request Common-Log-style lines to console (tracing
  target `access`) and optionally to a file
- **Server push** - a WebSocket at `/ws` streams metrics every 2s plus
  activity events, with auto-reconnect and connection-status toasts

## Quick start

Prerequisites: Rust (stable) and Node 18+.

```sh
# 1. Build the frontend (embedded by the Rust build)
cd ui && npm install && npm run build && cd ..

# 2. Build + run the single binary
cargo run --release
# → admin UI at http://127.0.0.1:9000
```

Or just `make build` / `make run`.

### Development (hot reload)

Run the two dev servers side by side:

```sh
cargo run                # terminal 1 — backend (admin plane on :9000)
cd ui && npm run dev     # terminal 2 — Vite on :5173, proxies /api and /ws
```

Open http://localhost:5173. In debug builds `rust-embed` reads `ui/dist` from
disk, so a production `npm run build` is picked up by a plain restart too.

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
| `data_plane.trusted_proxies` | `[]` | CIDRs whose `X-Forwarded-*` headers are trusted |
| `data_plane.shutdown_grace` | `30s` | Time in-flight requests get to finish on shutdown |
| `data_plane.listener` | one `http` listener on `0.0.0.0:8000` | Array of tables with `name` and `listen`; names and addresses must be unique |
| `admin.listen` | `127.0.0.1:9000` | Admin plane address; must differ from every listener (no authentication yet, keep it on loopback) |
| `logging.level` | `info` | Tracing filter (full directives allowed, e.g. `info,access=warn`) |
| `logging.access_log` | *(unset)* | Access-log file path; omit to disable the file |
| `ui.app_name` | `Fast Proxy` | Shown in the sidebar + browser title |
| `ui.tagline` | … | Shown under the app name |
| `ui.default_theme` | `auto` | `light` \| `dark` \| `auto` — used until the visitor picks |
| `ui.repo_url` | *(unset)* | Sidebar "Repository" link |

If the config file is missing, built-in defaults are used (with a warning).
Unknown keys are rejected, so a typo fails at startup instead of being ignored.
The data plane listeners are parsed and validated but not bound yet; that lands
with the proxy itself (M1 task D1).

## API

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | Liveness + version |
| GET | `/api/config` | UI bootstrap config (the `[ui]` table, camelCased) |
| GET | `/api/metrics` | Latest metrics snapshot |
| GET | `/ws` | WebSocket: server-push events |

Errors always look like:

```json
{ "error": { "code": "NOT_FOUND", "message": "subsystem 42 does not exist", "status": 404 } }
```

WebSocket events are JSON discriminated by `type`:

```json
{ "type": "metrics",  "data": { "cpu": 41.3, "memory": 58.0, "...": "…" } }
{ "type": "activity", "kind": "config", "message": "Route created", "timestampMs": 0 }
{ "type": "hello",    "message": "Connected to Fast Proxy v0.1.0", "timestampMs": 0 }
```

## Project structure

```
├── docs/                     ROADMAP.md · M1-PLAN.md · adr/
├── config.toml               server + UI configuration
├── src/
│   ├── main.rs               CLI (clap) + startup
│   └── config.rs             TOML config model
├── crates/
│   ├── fast-proxy-admin/     Axum admin plane: api/ · ws · events · error · embedded SPA
│   ├── fast-proxy-telemetry/ access log (metrics and traffic stats to come)
│   ├── fast-proxy-model/     validated domain types and the Config tree
│   ├── fast-proxy-routing/   Config -> Snapshot compiler, host and route matchers
│   ├── fast-proxy-config-service/  apply pipeline: validate, persist, swap snapshot
│   ├── fast-proxy-store/     SQLite storage, transactional writes, revisions, rollback
│   └── fast-proxy-dataplane/ hyper listeners and request pipeline (forwarding to come)
└── ui/
    ├── public/favicon.svg    the gear-and-bolt mark
    └── src/
        ├── lib/api.ts        typed client + ApiError
        ├── icons/            the icon pack (add icons here)
        ├── context/          Theme · Toast · Config · Live(WebSocket)
        ├── components/       layout, cards, chart, feed, controls
        ├── pages/            Dashboard · Settings · 404
        └── styles/           index.css (Tailwind v4 entry + theme tokens)
```

## Extending

**New endpoint** — add a handler under
`crates/fast-proxy-admin/src/api/` returning `AppResult<Json<T>>`, register it in
`crates/fast-proxy-admin/src/router.rs`, add a typed wrapper
in `ui/src/lib/api.ts`.

**New page** — create `ui/src/pages/X.tsx`, add a `<Route>` in `App.tsx` and a
nav item in `components/Sidebar.tsx` (plus a title in `Topbar.tsx`).

**New server-push event** — add a variant to `Event` in
`crates/fast-proxy-admin/src/events.rs`, broadcast it via `state.broadcast(…)`, handle its
`type` in `ui/src/context/LiveContext.tsx`.

**Theming** — the design tokens live at the top of `ui/src/styles/index.css`
(one block per mode, registered as Tailwind colors via `@theme inline`).
Change the accent or surfaces there; every `bg-surface` / `text-ink` /
`bg-accent` utility picks the new values up in both light and dark mode.

## Testing

```sh
cargo test                 # backend: unit + full HTTP router integration tests
cd ui && npm test          # frontend: Vitest + Testing Library
```

`cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check` are
also run in CI.

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
