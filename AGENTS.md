# AGENTS.md

Guidance for coding agents working in this repository.

## Project Overview

`rusty-template` is a single-binary fullstack template:

- Rust backend using Axum, Tokio, Clap, Serde, Tracing, and `rust-embed`.
- React frontend under `ui/`, built with Vite, TypeScript, Tailwind CSS v4, TanStack Router, and TanStack Query.
- The frontend build output is embedded into the Rust binary from `ui/dist`.

## Repository Layout

- `src/main.rs` wires CLI parsing, configuration, logging, application state, routes, and server startup.
- `src/config.rs` defines TOML configuration and CLI overrides.
- `src/error.rs` defines the shared API error envelope.
- `src/static_assets.rs` serves embedded SPA assets and fallback routing.
- `src/routes/` contains REST and WebSocket route handlers.
- `src/services/` contains metrics, task storage, and event broadcasting.
- `ui/src/lib/api.ts` is the typed frontend API client.
- `ui/src/context/` contains Theme, Toast, Config, and WebSocket live data providers.
- `ui/src/components/` contains reusable UI components.
- `ui/src/pages/` contains route pages.
- `ui/src/styles/index.css` contains Tailwind entrypoint and theme tokens.

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
- Add new REST handlers in `src/routes/api.rs`, register routes in `src/routes/mod.rs`, and expose typed frontend calls from `ui/src/lib/api.ts`.
- Add new WebSocket event types in `src/services/events.rs` and handle them in `ui/src/context/LiveContext.tsx`.
- Add new pages under `ui/src/pages/`, then update routing and navigation in the existing frontend structure.
- Keep UI styling aligned with the tokens in `ui/src/styles/index.css`; avoid hard-coded one-off colors when a token exists.
- Prefer existing component patterns in `ui/src/components/` before adding new abstractions.
- Do not commit generated artifacts such as `target/`, `ui/dist/`, or `ui/node_modules/` unless explicitly requested.

## Testing Expectations

- For backend changes, run at least `cargo test`; also run `cargo fmt --all -- --check` and `cargo clippy --all-targets -- -D warnings` when practical.
- For frontend changes, run `cd ui && npm test`; run `cd ui && npm run build` for type-checking and production bundle verification.
- For changes spanning backend and frontend contracts, test both sides.

## Configuration

- Runtime config defaults to `config.toml`.
- CLI flags can override config values such as host, port, log level, access log, and config path.
- UI boot configuration is served from `GET /api/config` based on the `[ui]` section of `config.toml`.

