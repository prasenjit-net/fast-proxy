# ADR 0001: SQLite via rusqlite for configuration storage

Status: proposed (needs owner confirmation). Plan reference: M1 D1, D2, D3.

## Context

The proxy persists subsystems, hosts, routes, upstreams, and revision history.
The data plane never reads the database; only the admin write path and startup
do. Write volume is tiny and human-driven. The binary must stay a single
self-contained file.

## Decision

Use SQLite in WAL mode through `rusqlite` with the `bundled` feature. One
`Store` handle owns one connection on a dedicated thread, driven by a channel,
so async code never blocks on the database. There are no migrations: a single
embedded `schema.sql` creates the tables on first open, and `PRAGMA
user_version` only detects an incompatible file.

## Consequences

- No system SQLite dependency; the C library is compiled into the binary.
- A synchronous API on its own thread is simpler than an async driver and
  fits a single-writer workload.
- Schema changes edit `schema.sql` directly, which is allowed while the
  project is undeployed (see `AGENTS.md`). Revisit when it is deployed.

## Alternatives considered

- `sqlx`: async and compile-time checked queries, but heavier and needs
  offline query metadata in CI; the workload does not need async.
- An embedded KV store (`redb`, `sled`): no SQL constraints, so uniqueness
  and foreign keys would move into our code.
