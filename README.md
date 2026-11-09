# rusty-template

[![CI](https://github.com/prasenjit-net/rusty-template/actions/workflows/ci.yml/badge.svg)](https://github.com/prasenjit-net/rusty-template/actions/workflows/ci.yml)

A **single-binary fullstack template**: an [Axum](https://github.com/tokio-rs/axum)
REST + WebSocket backend with a React (Vite + TypeScript) SPA embedded straight
into the executable. Build once, ship one file.

## Features

- **REST backend** — Axum with a typed error framework (`AppError` → consistent
  JSON error envelope on every endpoint)
- **Embedded SPA** — `ui/dist` is compiled into the binary with `rust-embed`;
  deep links work via the SPA fallback, fingerprinted assets get immutable
  cache headers, unknown `/api/*` paths still return JSON 404s
- **Greptile-inspired theme** — light-gray canvas with a faint emerald grid,
  dark purple-slate ink, mint/emerald accents, and monospace meta text, built
  on **Tailwind CSS v4**, plus a bespoke 24×24 stroke **icon pack**
- **Light / dark / auto theme** — applied before first paint, follows the OS in
  auto mode, visitor override persisted in `localStorage`
- **Dashboard UI** — stat tiles with sparklines, a live two-series chart with
  crosshair + tooltip (colors validated for contrast and color-vision safety in
  both modes), activity feed, tasks card
- **Example service** — an in-memory tasks CRUD (`/api/tasks`) demonstrating
  validation, 404s, and activity broadcasting
- **Example page** — a component gallery (buttons, badges, forms, alerts,
  table, progress, skeletons) with live error-pipeline demos
- **Responsive + collapsible sidebar** — the topbar hamburger toggles the
  sidebar between icon+text and an icon-only rail on desktop (choice
  persisted), and opens it as an overlay drawer on mobile; layouts collapse
  down to phone widths
- **UI configured by the backend** — the SPA boots from `GET /api/config`,
  which serves the `[ui]` section of `config.toml` (app name, tagline, default
  theme, repo link)
- **TOML configuration + CLI overrides** — every value in `config.toml` can be
  overridden with a flag (`--port`, `--host`, `--log-level`, `--access-log`,
  `--no-access-log`, `--config <path>`)
- **Access log** — per-request Common-Log-style lines to console (tracing
  target `access`) and optionally to a file
- **Error handling end to end** — backend errors arrive in the UI as
  notification bubbles (toasts) carrying the server's error code; render
  crashes are caught by nested error boundaries
- **Server push** — a WebSocket at `/ws` streams metrics every 2s plus
  activity events, with auto-reconnect and connection-status toasts
- **Unique favicon** — a mint-on-slate gear-and-bolt SVG, mirrored by the
  in-app logo

## Quick start

Prerequisites: Rust (stable) and Node 18+.

```sh
# 1. Build the frontend (embedded by the Rust build)
cd ui && npm install && npm run build && cd ..

# 2. Build + run the single binary
cargo run --release
# → http://127.0.0.1:8080
```

Or just `make build` / `make run`.

### Development (hot reload)

Run the two dev servers side by side:

```sh
cargo run                # terminal 1 — backend on :8080
cd ui && npm run dev     # terminal 2 — Vite on :5173, proxies /api and /ws
```

Open http://localhost:5173. In debug builds `rust-embed` reads `ui/dist` from
disk, so a production `npm run build` is picked up by a plain restart too.

## CLI

```
rusty-template [OPTIONS]

  -c, --config <CONFIG>        Path to the TOML configuration file [default: config.toml]
      --host <HOST>            Override [server].host
  -p, --port <PORT>            Override [server].port
      --log-level <LOG_LEVEL>  Override [logging].level (trace, debug, info, warn, error)
      --access-log <PATH>      Override [logging].access_log file path
      --no-access-log          Disable the access log file entirely
```

## Configuration (`config.toml`)

| Key | Default | Meaning |
|---|---|---|
| `server.host` | `127.0.0.1` | Bind address |
| `server.port` | `8080` | Bind port |
| `logging.level` | `info` | Tracing filter (full directives allowed, e.g. `info,access=warn`) |
| `logging.access_log` | *(unset)* | Access-log file path; omit to disable the file |
| `ui.app_name` | `Rusty Template` | Shown in the sidebar + browser title |
| `ui.tagline` | … | Shown under the app name |
| `ui.default_theme` | `auto` | `light` \| `dark` \| `auto` — used until the visitor picks |
| `ui.repo_url` | *(unset)* | Sidebar "Repository" link |

If the config file is missing, built-in defaults are used (with a warning).

## API

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | Liveness + version |
| GET | `/api/config` | UI bootstrap config (the `[ui]` table, camelCased) |
| GET | `/api/metrics` | Latest metrics snapshot |
| GET | `/api/tasks` | List tasks |
| POST | `/api/tasks` | Create (`{"title": "…"}`; empty title → 400) |
| POST | `/api/tasks/{id}/toggle` | Toggle done |
| DELETE | `/api/tasks/{id}` | Delete (204) |
| GET | `/api/error-demo?kind=…` | Always fails (`internal`, `bad-request`, `not-found`) |
| GET | `/ws` | WebSocket: server-push events |

Errors always look like:

```json
{ "error": { "code": "NOT_FOUND", "message": "task 42 does not exist", "status": 404 } }
```

WebSocket events are JSON discriminated by `type`:

```json
{ "type": "metrics",  "data": { "cpu": 41.3, "memory": 58.0, "...": "…" } }
{ "type": "activity", "kind": "task", "message": "Task \"x\" created", "timestampMs": 0 }
{ "type": "hello",    "message": "Connected to Rusty Template v0.1.0", "timestampMs": 0 }
```

## Project structure

```
├── config.toml               server + UI configuration
├── src/
│   ├── main.rs               CLI (clap) + startup
│   ├── config.rs             TOML config model
│   ├── error.rs              AppError → JSON error envelope
│   ├── access_log.rs         access-log middleware (console + file)
│   ├── static_assets.rs      embedded SPA + fallback routing
│   ├── routes/               api.rs (REST) · ws.rs (server push)
│   └── services/             metrics.rs · tasks.rs · events.rs
└── ui/
    ├── public/favicon.svg    the gear-and-bolt mark
    └── src/
        ├── lib/api.ts        typed client + ApiError
        ├── icons/            the icon pack (add icons here)
        ├── context/          Theme · Toast · Config · Live(WebSocket)
        ├── components/       layout, cards, chart, feed, controls
        ├── pages/            Dashboard · Components · Settings · 404
        └── styles/           index.css (Tailwind v4 entry + theme tokens)
```

## Extending

**New endpoint** — add a handler in `src/routes/api.rs` returning
`AppResult<Json<T>>`, register it in `src/routes/mod.rs`, add a typed wrapper
in `ui/src/lib/api.ts`.

**New page** — create `ui/src/pages/X.tsx`, add a `<Route>` in `App.tsx` and a
nav item in `components/Sidebar.tsx` (plus a title in `Topbar.tsx`).

**New server-push event** — add a variant to `Event` in
`src/services/events.rs`, broadcast it via `state.broadcast(…)`, handle its
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
