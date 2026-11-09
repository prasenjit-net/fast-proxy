# AGENTS.md

Guidance for coding agents working in this repository.

## Project Overview

`fast-proxy` is a fast, realtime-configurable ingress server and reverse proxy (see `docs/ROADMAP.md`). It currently sits on a single-binary fullstack scaffold:

- Rust backend using Axum, Tokio, Clap, Serde, Tracing, and `rust-embed`.
- React frontend under `ui/`, built with Vite, TypeScript, Tailwind CSS v4, TanStack Router, and TanStack Query.
- The frontend build output is embedded into the Rust binary from `ui/dist`.

## Repository Layout

Cargo workspace; the root package is the `fast-proxy` binary.

- `src/main.rs` wires CLI parsing, configuration, logging, and server startup.
- `src/config.rs` defines TOML configuration and CLI overrides.
- `crates/fast-proxy-admin/` is the Axum admin plane: `router.rs`, `api/` (one
  file per resource), `ws.rs`, `events.rs`, `error.rs` (shared API error
  envelope), `static_assets.rs` (embedded SPA from `ui/dist`), `state.rs`.
- `crates/fast-proxy-telemetry/` holds the access log (metrics and traffic
  stats land here in M1).
- `crates/fast-proxy-model/`, `-store/`, `-routing/`, `-dataplane/` are
  skeletons filled in by the M1 plan (`docs/M1-PLAN.md`).
- `docs/adr/` records design decisions.
- `ui/src/lib/api.ts` is the typed frontend API client.
- `ui/src/context/` contains Theme, Toast, Config, and WebSocket live data providers.
- `ui/src/components/` contains reusable UI components.
- `ui/src/pages/` contains route pages.
- `ui/src/styles/index.css` contains Tailwind entrypoint and theme tokens.

## Development Rules

These rules are set by the project owner and apply to every session.

### Workflow

- **Never commit or push unless the owner explicitly says so.** When a task is
  done, stop and report. The owner does a manual check first, then asks for the
  commit and push.
- Plans and roadmap live in `docs/` (`ROADMAP.md`, `M1-PLAN.md`). Keep them
  current: tick tasks as they land and record decisions.

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

Development servers:

- Backend: `cargo run` on `127.0.0.1:8080`.
- Frontend: `cd ui && npm run dev` on `localhost:5173`, proxying `/api` and `/ws` to the backend.

## Implementation Notes

- Keep API responses using the existing `AppError` / JSON error envelope pattern.
- Add new REST handlers under `crates/fast-proxy-admin/src/api/`, register routes in `crates/fast-proxy-admin/src/router.rs`, and expose typed frontend calls from `ui/src/lib/api.ts`.
- Add new WebSocket event types in `crates/fast-proxy-admin/src/events.rs` and handle them in `ui/src/context/LiveContext.tsx`.
- Add new pages under `ui/src/pages/`, then update routing and navigation in the existing frontend structure.
- Keep UI styling aligned with the tokens in `ui/src/styles/index.css`; avoid hard-coded one-off colors when a token exists.
- Prefer existing component patterns in `ui/src/components/` before adding new abstractions.
- Do not commit generated artifacts such as `target/`, `ui/dist/`, or `ui/node_modules/` unless explicitly requested.

## Testing Expectations

- For backend changes, run at least `cargo test`; also run `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` when practical.
- For frontend changes, run `cd ui && npm test`; run `cd ui && npm run build` for type-checking and production bundle verification.
- For changes spanning backend and frontend contracts, test both sides.
- Report test coverage (see Development Rules) when finishing a task.

## Configuration

- Runtime config defaults to `config.toml`.
- CLI flags can override config values such as host, port, log level, access log, and config path.
- UI boot configuration is served from `GET /api/config` based on the `[ui]` section of `config.toml`.

