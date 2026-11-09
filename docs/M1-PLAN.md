# M1 Plan: Core proxy and realtime config

Status: draft v0.1 (2026-11-09). Parent document: [ROADMAP.md](ROADMAP.md).
Task sizes are relative effort (S under half a day, M about 1-2 days, L about
3-5 days), not commitments. Tick the checkboxes as tasks land.

## 1. Goal and exit criteria

Turn the scaffold into a working reverse proxy that is configured live from the
admin UI.

**Exit criteria** (from the roadmap):

1. Start the binary with a data plane config file and an **empty DB**.
2. In the admin UI, create two subsystems (two host names), add prefix routes
   that point at different backends.
3. Traffic through the data plane is routed correctly **immediately**, with no
   restart.
4. Edits apply live and do not drop in-flight requests.
5. All configuration survives a restart.
6. Metrics, access log and the dashboard show real traffic per subsystem.

## 2. Scope

### In scope

- New config file model (`[data_plane]`, `[admin]`, `[logging]`, `[ui]`).
- SQLite storage for subsystems, hosts, routes, upstreams, revision history.
- Immutable compiled snapshot, swapped lock-free with `arc-swap`.
- Plain HTTP/1.1 data plane on hyper: host -> subsystem -> prefix route ->
  upstream, streaming bodies, WebSocket/upgrade passthrough.
- Forwarded headers, hop-by-hop handling, request IDs, timeouts, error mapping.
- Admin REST API and admin UI for subsystems, hosts, routes, upstreams,
  revisions and rollback, plus a "try a request" resolver.
- Baseline Prometheus metrics, enriched access log, live traffic stats.
- Graceful shutdown with connection draining.
- Removal of the scaffold demos.

### Out of scope (later milestones)

| Item | Milestone |
|---|---|
| Multiple endpoints per upstream, load balancing, health checks, retries | M2 |
| TLS listeners, SNI, HTTP/2 | M3 |
| ACME | M4 |
| Maintenance mode, admin authentication, audit log, config import/export | M5 |
| Request sampling | M6 |
| Rate and connection limits | M7 |

The admin plane has **no authentication in M1**. It binds to loopback by
default and logs a loud warning if configured otherwise (task A5).

## 3. Design decisions for M1

Items marked **(confirm)** are recommendations that need your sign-off before
the dependent tasks start.

| # | Decision | Recommendation |
|---|---|---|
| D1 | DB engine and crate | SQLite in WAL mode via `rusqlite` with the `bundled` feature (no system dependency, simple sync API). Confirmed. |
| D2 | Schema management | No migrations (project is not deployed). A single embedded `schema.sql` creates the tables on first open; schema changes edit that file directly. `PRAGMA user_version` is set only to detect an incompatible DB file. |
| D3 | DB access from async | One `Store` handle owning a single connection on a dedicated thread, driven by a channel. The data plane hot path **never** touches the DB. |
| D4 | Upstream shape in M1 | Schema already has `upstream_group` and `upstream_endpoint`, but validation allows **exactly one endpoint** per group. M2 lifts this with no migration. |
| D5 | HTTP stack | `hyper` 1.x + `hyper-util` (server auto builder, legacy client with pooling). Axum stays admin-only. |
| D6 | Metrics library | `prometheus` crate (simple, text exposition built in) unless the T0.1 spike shows a problem. Confirmed. |
| D7 | Matcher implementation | Own small implementation: `HashMap` for exact hosts, suffix `HashMap` for wildcards, segment-aware longest-prefix for routes. Benchmark before considering a crate. |
| D8 | IDs | Integer primary keys internally; names are unique and used in URLs of the UI and as metric labels. Renaming a subsystem changes its label (documented). |
| D9 | Concurrent admin edits | Last write wins in M1. Every resource returns `updatedAt` so optimistic concurrency can be added later. |
| D10 | Forwarded headers | Default: do **not** trust incoming `X-Forwarded-*`; overwrite them. A `trusted_proxies` CIDR list in the data plane config opts in to appending. |
| D11 | Host header to upstream | Preserved by default; per-route option `preserve_host = false` rewrites it to the endpoint address. |
| D12 | Revision history | Each commit stores the full resulting config as JSON (small in M1). Rollback re-applies a stored revision as a new revision. Keep the last 100. |

## 4. Target architecture

```
                   config.toml (data plane)                    SQLite file
                           |                                       ^
                           v                                       | (D3: store thread)
   +-------------------- main ------------------------------+      |
   |                                                         |      |
   |  data plane (hyper)            admin plane (axum)       |      |
   |  listeners[]                   127.0.0.1:9000           |      |
   |     |                          REST + WS + UI + /metrics|      |
   |     v                                  |                |      |
   |  load Snapshot (arc-swap) <--- swap ---+-- apply -------+------+
   |     |            ^                     |  (validate, commit, compile, swap)
   |  host -> subsystem -> route -> upstream
   |     |
   |  forward via pooled client  --> backends
   +---------------------------------------------------------+
```

Layout follows the project's development rules (see `AGENTS.md`): a Cargo
workspace of small crates with one-way dependencies, many small well-named
files, doc comments, and tests in every crate.

```
Cargo.toml                    workspace root + the `fast-proxy` binary package
src/                          binary crate: wiring only
  main.rs                     CLI, startup, shutdown wiring
  config.rs                   data plane config file model
crates/
  fast-proxy-model/           domain types + pure validation (no I/O)
    src/ subsystem.rs · host_pattern.rs · route.rs · upstream.rs · validation.rs
  fast-proxy-store/           SQLite persistence (depends on model)
    src/ store.rs · schema.sql · repository/*.rs · revision.rs · error.rs
  fast-proxy-routing/         snapshot + matchers (depends on model; pure, no I/O)
    src/ snapshot.rs · holder.rs · host_table.rs · route_table.rs · compile.rs
  fast-proxy-dataplane/       hyper proxy (depends on routing, telemetry)
    src/ server.rs · handler.rs · headers.rs · client.rs · upgrade.rs · responses.rs
  fast-proxy-telemetry/       metrics, access log, live traffic stats
    src/ metrics.rs · access_log.rs · traffic_stats.rs
  fast-proxy-admin/           Axum admin API, WebSocket, static UI
    (depends on model, store, routing, telemetry)
    src/ router.rs · api/*.rs (one file per resource) · ws.rs · static_assets.rs · error.rs
tests/                        end-to-end scenarios against the whole binary
ui/src/
  pages/                      Subsystems · SubsystemDetail · Revisions · Dashboard · Resolver
  lib/api.ts                  typed client for the new endpoints
```

Dependency direction: `model` <- `store`, `routing`; `routing` + `telemetry`
<- `dataplane`; `model` + `store` + `routing` + `telemetry` <- `admin`; all
<- the binary. The config apply pipeline (R5) lives in a small
`fast-proxy-config-service` crate or in `routing`, decided in P1, as long as
`dataplane` never depends on `store`.

## 5. Data model (schema v1)

```sql
CREATE TABLE subsystem (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,          -- [a-z0-9-], 1..63
  enabled     INTEGER NOT NULL DEFAULT 1,
  description TEXT NOT NULL DEFAULT '',
  created_at  INTEGER NOT NULL,              -- unix ms
  updated_at  INTEGER NOT NULL
);

CREATE TABLE subsystem_host (
  id           INTEGER PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  pattern      TEXT NOT NULL UNIQUE           -- lowercase; exact or "*.suffix"; unique across ALL subsystems
);

CREATE TABLE upstream_group (
  id           INTEGER PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  name         TEXT NOT NULL,
  UNIQUE (subsystem_id, name)
);

CREATE TABLE upstream_endpoint (
  id        INTEGER PRIMARY KEY,
  group_id  INTEGER NOT NULL REFERENCES upstream_group(id) ON DELETE CASCADE,
  addr      TEXT NOT NULL,                    -- host:port
  weight    INTEGER NOT NULL DEFAULT 1        -- unused until M2
);

CREATE TABLE route (
  id                INTEGER PRIMARY KEY,
  subsystem_id      INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  prefix            TEXT NOT NULL,            -- "/" or "/seg[/seg]*", no trailing slash
  upstream_group_id INTEGER NOT NULL REFERENCES upstream_group(id) ON DELETE RESTRICT,
  strip_prefix      INTEGER NOT NULL DEFAULT 0,
  upstream_prefix   TEXT NOT NULL DEFAULT '', -- prepended to the forwarded path
  preserve_host     INTEGER NOT NULL DEFAULT 1,
  connect_timeout_ms  INTEGER NOT NULL DEFAULT 5000,
  response_timeout_ms INTEGER NOT NULL DEFAULT 30000,
  enabled           INTEGER NOT NULL DEFAULT 1,
  UNIQUE (subsystem_id, prefix)
);

CREATE TABLE revision (
  version    INTEGER PRIMARY KEY,             -- monotonically increasing
  created_at INTEGER NOT NULL,
  summary    TEXT NOT NULL,                   -- e.g. "route created: /api in shop"
  config     TEXT NOT NULL                    -- full JSON of resulting config
);
```

Cross-table rules enforced in code (and by the test suite), because SQLite
cannot express them as constraints:

- A route's `upstream_group_id` must belong to the **same subsystem**.
- Each upstream group has exactly one endpoint in M1 (D4).
- Deleting a group still referenced by a route is rejected (409).

## 6. Tasks

Task IDs are referenced by the dependency table in section 7. Workstream
letters: **P** groundwork, **C** config, **S** storage, **R** snapshot and
routing, **D** data plane, **T** telemetry, **A** admin API, **U** admin UI,
**Q** quality, **W** docs.

### Workstream P: Groundwork

- [~] **P1 (S) Short decision notes.** Write `docs/adr/` notes for D1, D5, D6,
  D7 (a page each). Spike D6 (metrics crate) and the hyper client pool in a
  scratch branch to confirm they fit: pool reuse works, per-request timeouts
  are controllable, body streaming does not buffer.
  *Done when:* notes are merged and the open **(confirm)** items are resolved.
  *Status:* ADRs 0001-0004 are written in `docs/adr/`. D1 (rusqlite) and D6
  (prometheus crate) were confirmed by the owner on 2026-11-09. The hyper
  client spike is done (results in ADR 0002). Still open: the metrics crate
  spike, folded into T1.
- [x] **P2 (M) Remove scaffold demos.** Delete the tasks service and routes,
  `/api/error-demo`, the Components page, `TasksCard`, their tests, and the
  related README sections. Keep access log, events, and the WebSocket plumbing.
  *Done when:* `cargo test`, `npm test`, `npm run build` pass with the demos
  gone and the sidebar shows only Dashboard and Settings.
- [x] **P3 (M) Convert to a Cargo workspace** with the crate layout in section
  4. Move existing code without behavior changes (`routes/` and
  `static_assets` -> `fast-proxy-admin`, `access_log` -> `fast-proxy-telemetry`).
  Create the other crates as documented, empty skeletons. Set up coverage
  measurement (`cargo llvm-cov`) and record the starting number.
  *Done when:* the app behaves identically and tests pass.
  *Result:* `access_log` was split into a telemetry `AccessLog` and an admin
  middleware so telemetry does not depend on axum. The admin crate takes
  `UiConfig` and the version through `AdminState`. Starting coverage
  (`cargo llvm-cov --workspace`): 63% lines.

### Workstream C: Configuration

- [x] **C1 (M) New config model** in `config.rs`:

  ```toml
  [data_plane]
  state_dir = "./state"
  database  = "./state/fast-proxy.db"      # default: <state_dir>/fast-proxy.db
  trusted_proxies = []                     # CIDRs, D10
  shutdown_grace = "30s"
  [[data_plane.listener]]
  name = "http"
  listen = "0.0.0.0:8000"

  [admin]
  listen = "127.0.0.1:9000"
  ```

  Remove `[server]`. Keep `[logging]` and `[ui]`. Keep
  `deny_unknown_fields`, so a `tls` key on a listener is rejected until M3.
  Validate: at least one listener, unique listener names and addresses, admin
  address differs from every listener, `state_dir` creatable, duration parsing.
  *Done when:* unit tests cover defaults, missing sections, unknown fields, and
  each validation failure with a clear message.
- [x] **C2 (S) CLI overrides.** Replace `--host/--port` with `--admin-listen`,
  `--database`, `--state-dir`; keep `--config`, `--log-level`, `--access-log`,
  `--no-access-log`. Update `Cli::apply`.
- [x] **C3 (S) Update `config.toml`, README config table and CLI section,
  and AGENTS.md** for the new keys. Default ports: data plane `8000`, admin
  `9000` (was `8080`); update the Vite dev proxy in `ui/vite.config.ts` to the
  admin port.
  *Done when:* a fresh clone follows the README and starts successfully.

### Workstream S: Storage

- [x] **S1 (M) SQLite store skeleton.** Add `rusqlite` (bundled). `Store`
  handle running a dedicated thread with a command channel (D3). On open:
  create parent dir, enable WAL, `foreign_keys=ON`, `busy_timeout`, apply
  the embedded `schema.sql` on an empty DB (D2). Refuse to start if the DB
  `user_version` does not match the binary.
  *Done when:* tests open a temp DB, create the schema from empty, reopen
  idempotently, and reject a mismatched `user_version`.
- [x] **S2 (S) Schema** (section 5) as `schema.sql` in the store crate.
- [x] **S3 (M) Domain models and validation** (built in the `model` crate, one
  file per type, not `store/models.rs`), pure and heavily unit tested:
  - subsystem name: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`
  - host pattern: lowercased; exact DNS name or `*.` + DNS name (a single
    leading wildcard label only); no port, no trailing dot; IP literals
    rejected in M1
  - route prefix: starts with `/`, no `?`/`#`/`//`/`..`/control characters,
    normalized to no trailing slash except `/`
  - `upstream_prefix`: same rules as prefix, may be empty
  - endpoint `addr`: `host:port`, port 1-65535; hostname or IP
  - timeouts: bounded (for example 100 ms to 10 min)
  *Done when:* table-driven tests list accepted and rejected inputs, each
  rejection producing a field-specific message.
- [x] **S4 (L) Repository operations** with transactions, each write also
  appending a `revision` row (D12) and returning the new version:
  - subsystems: create, get, list, update, delete
  - hosts: add, remove (uniqueness enforced across subsystems)
  - upstream groups and their single endpoint: create, update, delete
  - routes: create, update, delete (same-subsystem rule, RESTRICT on group)
  - `load_all()` returning the full config for compilation and revisions
  Error type: `StoreError::{NotFound, Conflict, Invalid(fields), Internal}`
  mapped to `AppError` (404, 409, 400, 500).
  *Done when:* each operation has success and failure tests, including
  uniqueness conflicts, FK/RESTRICT behavior, cascade delete, and revision rows.
  *Result and notes:*
  - Operations are plain functions over `&Connection` in `repository/`; all
    writes go through `Database::write(mutate, check)`, which runs the mutation
    in a transaction, passes the would-be `Config` to `check`, and only then
    records the revision and commits. `check` is where R5 plugs in `compile`.
  - `route.upstream_group_id` uses NO ACTION instead of RESTRICT: same effect
    for deleting a group, but deleting a whole subsystem (cascade) still works.
  - `StoreError` to `AppError` mapping (404/409/400/500) is done with the admin
    API in A1; `created_at`/`updated_at` are stored but not yet exposed (A1).
  - Host ids are not part of `Config`; `hosts::list` returns `(HostId, pattern)`.
- [x] **S5 (M) Revisions and rollback.** `list_revisions`, `get_revision`,
  `rollback(version)`: replace live tables with the stored config in one
  transaction and append a new revision "rollback to vN". Prune to the last 100.
  *Done when:* a test creates, edits, deletes, rolls back to each prior
  version and compares `load_all()` to the stored config.

### Workstream R: Snapshot and routing

- [x] **R1 (M) Compiled snapshot types** (`snapshot/`): `Snapshot { version,
  hosts: HostTable, subsystems: HashMap<SubsystemId, CompiledSubsystem> }`;
  `CompiledSubsystem { name, enabled, routes: RouteTable, upstreams }` where
  routes hold resolved endpoint addresses and timeouts.
  `compile(&Config) -> Result<Snapshot, CompileError>`; deterministic and pure.
- [x] **R2 (M) Host matcher.** Exact match first, then the longest matching
  wildcard suffix. Normalization: lowercase, strip port, strip one trailing dot,
  handle bracketed IPv6 `Host` values (no match, no panic). Missing or invalid
  Host returns "no subsystem".
  *Done when:* unit tests plus a property test (a generated exact host always
  beats a wildcard that also matches; the longest wildcard wins; matching is
  case-insensitive).
- [x] **R3 (M) Route matcher.** Longest prefix wins on **segment boundaries**
  (`/api` matches `/api` and `/api/x`, not `/apix`); `/` is the catch-all;
  disabled routes are skipped at compile time. Returns the route plus the
  remaining path after the prefix.
  *Done when:* unit and property tests (result is always the longest matching
  prefix, equals a brute-force reference implementation).
- [x] **R4 (S) Snapshot holder.** `SnapshotHolder` wrapping
  `arc-swap::ArcSwap<Snapshot>` with `load()` and `swap()`; the initial value
  is an empty snapshot version 0.
- [x] **R5 (M) Apply pipeline.** One `ConfigService` owning a writer mutex so DB
  commit order equals swap order:
  1. take the writer lock
  2. run the store mutation in a transaction **without committing**
  3. load the would-be full config and `compile` it (validates cross-table
     rules and compile errors)
  4. commit (writes the revision row)
  5. `swap` the new snapshot
  6. broadcast a `config_changed` event
  If step 3 fails the transaction is rolled back and the old snapshot stays.
  On startup: open the store, `load_all`, `compile`, set the initial snapshot.
  *Done when:* tests show an invalid change leaves both the DB and the live
  snapshot untouched, and a concurrent reader never observes a partial state.
  *Result:* lives in the new `fast-proxy-config-service` crate (decided here:
  it keeps `routing` free of the store and `dataplane` free of both). The
  pipeline uses `Database::write`'s `check` hook, so steps 2-4 are one store
  transaction. It broadcasts a `ConfigChanged` on its own channel; bridging that
  to the admin WebSocket `config_changed` event is part of T3.

### Workstream D: Data plane

- [x] **D1 (M) Listeners and accept loop.** Bind each configured listener with
  `hyper-util` `server::conn::auto::Builder` (HTTP/1 only in M1). Track open
  connections with a counter and a shutdown signal (`tokio_util`
  `CancellationToken` or a watch channel). Set sane defaults: header read
  timeout, max header size, keep-alive.
  *Done when:* a bound port serves a hello response, and a bind failure aborts
  startup with a clear error.
  *Result:* `bind` is all-or-nothing and names the failing listener. The
  connection counter and the shutdown token (stop accepting) are in; draining
  open connections is D6. HTTP/1 only, with upgrades enabled for D5.
- [x] **D2 (M) Request pipeline** (`dataplane/handler.rs`): load snapshot once
  -> host match -> subsystem checks -> route match -> forward. Fixed responses
  (`responses.rs`, plain text or JSON by `Accept`):
  - no subsystem for host: 404 `unknown host`
  - subsystem disabled: 503
  - no route: 404 from the subsystem
  - upstream connect failure: 502; upstream timeout: 504
  - Each includes an `x-request-id`.
  *Done when:* integration tests assert status and body for each case.
  *Result:* the pipeline calls a `Forward` trait (implemented by the upstream
  client in D4; tests use a fake). `FixedResponse` renders all five fixed
  responses, including 502/504, which D4 will return on upstream failures.
  The 404/503/404 cases are tested end to end over TCP.
- [x] **D3 (M) Header handling** (`headers.rs`): strip hop-by-hop headers and any
  named in `Connection` (RFC 9110), both directions, while preserving
  `Upgrade` for upgrades (D6 task); set `X-Forwarded-For/Proto/Host` per D10;
  honor `preserve_host` (D11); generate `x-request-id` if absent and propagate
  it upstream and in the response.
  *Done when:* unit tests on header maps and an integration test with an echo
  upstream confirm what the backend actually receives.
  *Status:* done; `Upgrade` is kept for upgrade requests (D5). `trusted_proxies` entries are parsed by `TrustedProxies::parse`;
  the binary must call it at startup (D7) so a bad CIDR aborts with a clear error.
- [x] **D4 (L) Upstream client and forwarding** (`client.rs`): pooled hyper-util
  client, per-route connect and response-header timeouts, request and response
  bodies streamed (no buffering), `100-continue` and chunked bodies handled,
  client disconnect cancels the upstream request. Build the upstream URI from
  the matched route (`strip_prefix`, `upstream_prefix`, query string preserved
  byte for byte). Map errors to 502/504.
  *Done when:* tests cover GET, POST with large streamed body, chunked
  response, slow upstream (504), refused connection (502), and path/query
  rewriting cases.
  *Result:* all covered end to end (64 MiB body, not hundreds of MB). A connect
  timeout also maps to 504; that mapping is unit tested but not exercised
  against an unroutable address.
- [x] **D5 (M) Upgrade/WebSocket passthrough** (`upgrade.rs`): forward the
  upgrade request, relay the `101` response, then copy bytes in both
  directions until either side closes.
  *Done when:* an integration test completes a WebSocket echo through the proxy.
  *Result:* tested with a real WebSocket server and client (`tokio-tungstenite`,
  test-only). The proxy takes the client's side of the socket before sending the
  request, and on a `101` relays bytes with `copy_bidirectional`. If the upstream
  declines, the response is proxied normally. An `Upgrade` header without
  `Connection: upgrade` is stripped like any hop-by-hop header. Tunnels hold the
  connection counter, so draining waits for them.
- [x] **D6 (M) Graceful shutdown and draining.** On signal: stop accepting,
  let in-flight requests finish up to `shutdown_grace`, then close remaining
  connections. Admin plane shuts down after the data plane.
  *Done when:* a test holds a slow request open during shutdown and sees it
  complete, and another test sees forced close after the grace period.
  *Result:* `DataPlane::stopped(grace)` stops accepting, closes idle keep-alive
  connections at once (hyper `graceful_shutdown`), waits for in-flight requests
  and tunnels up to `grace`, then force-closes the rest and reports how many
  (`DrainReport`). `startup.rs` stops the admin plane only after the data plane
  has drained. Also tested on the real binary with a real SIGINT. Note: an open
  admin `/ws` does not hold up shutdown (hyper hands upgraded sockets off), which
  `tests/exit_criteria.rs` pins down.
- [x] **D7 (S) Startup wiring in `main.rs`:** parse config, open store, build
  initial snapshot, start the admin listener, then the data plane listeners;
  log a clear summary (listeners, admin URL, DB path, snapshot version).
  *Done when:* running with an empty state dir creates the DB and starts both
  planes.
  *Result:* `src/startup.rs` binds every listener (and the admin address)
  before serving, validates `trusted_proxies`, and logs listeners, admin URL,
  DB path and config version. Fatal startup errors now go to stderr. The
  binary-level tests in `tests/exit_criteria.rs` cover exit criteria 1-5.

### Workstream T: Telemetry

- [x] **T1 (M) Metrics** (`telemetry/metrics.rs`), exposed at `GET /metrics` on
  the admin listener only:
  - `fastproxy_requests_total{subsystem,route,status_class}`
  - `fastproxy_request_duration_seconds{subsystem,route}` (histogram)
  - `fastproxy_upstream_errors_total{subsystem,kind}` (connect, timeout, other)
  - `fastproxy_active_connections` and `fastproxy_active_requests`
  - `fastproxy_config_version`, `fastproxy_config_reloads_total{outcome}`
  Label rules: values come only from config names; unmatched traffic uses
  `subsystem="_unmatched"`, `route="_none"`. Never use raw paths or hosts.
  *Done when:* an integration test scrapes `/metrics` after known traffic and
  asserts the counters.
  *Result:* `Metrics` in the telemetry crate wraps the `prometheus` crate (ADR
  0003), so the library can be swapped without touching the data plane. Counters
  are recorded when the response body finishes (a wrapper body counts bytes and
  reports exactly once, including when the client leaves mid-response). The
  route label is the route's prefix (config-derived); tests assert that raw
  hosts and paths never become labels. Labelled families only appear in the
  exposition once they have a sample. A request cancelled before its response
  head exists is not counted. The hot-path cost of label lookups is not
  measured yet; Q3 will show it.
- [x] **T2 (M) Access log for the data plane.** Generalize the existing
  middleware into a function usable by both planes. Data plane lines add
  `subsystem`, `route`, `upstream`, `request_id`, `upstream_ms`, `total_ms`,
  `bytes_out`. Keep file plus console (tracing target `access`). Admin API
  requests keep a simpler line or are logged to the same sink with
  `plane=admin`.
  *Done when:* a test reads the log file after requests and parses the fields.
  *Result:* one sink for both planes, written by a background task so recording
  never waits on the disk (`flush()` is called at shutdown). Lines are a
  Common-Log-style request part followed by `key=value` fields: admin lines
  have `plane=admin total_ms=`; data plane lines add `subsystem`, `route`,
  `upstream`, `request_id`, `upstream_ms`, `bytes_out` (`-` where not
  applicable). Client-supplied values are escaped so they cannot forge fields
  or lines. The `elapsed` tail of the old format became `total_ms`.
- [x] **T3 (M) Live traffic stats.** Replace the fake CPU/memory sampler in
  `services/metrics.rs` with real counters: per subsystem requests per second,
  error rate, active connections, plus global totals. A background task samples
  counters every 2 s and broadcasts a `traffic` WebSocket event. Add the
  `config_changed` event type.
  *Done when:* the WebSocket test client receives `traffic` events whose
  numbers match generated traffic.
  *Result:* the fake CPU/memory sampler is gone. Every 2 s the sampler reads the
  cumulative request counters from the metrics registry (no extra hot-path
  state), computes per-subsystem and total requests/s and 5xx error rate, stores
  the latest sample (`GET /api/traffic`, replacing `/api/metrics`) and pushes a
  `traffic` event. A `configChanged` event plus a `config` activity entry is
  sent for every applied change (the bridge from `ConfigService`). The first
  sample is taken after a full window; an e2e test caught the sampler
  reporting a ~1 ms window at startup. The dashboard was updated to the new
  contract (U6 will add the per-subsystem table).

### Workstream A: Admin API

All endpoints return JSON and use the existing `AppError` envelope. The error
envelope gains an optional `fields: [{ field, message }]` array for validation
failures (backward compatible). Every write returns the resource and the new
config `version`.

- [x] **A1 (M) Subsystems and hosts** (`admin/api/subsystems.rs`):
  `GET/POST /api/subsystems`, `GET/PUT/DELETE /api/subsystems/{id}`,
  `POST /api/subsystems/{id}/hosts`,
  `DELETE /api/subsystems/{id}/hosts/{host_id}`. List responses include host
  patterns and route counts.
- [x] **A2 (M) Upstreams and routes** (`admin/api/{upstreams,routes}.rs`):
  `GET/POST /api/subsystems/{id}/upstreams`, `PUT/DELETE .../upstreams/{uid}`;
  `GET/POST /api/subsystems/{id}/routes`, `PUT/DELETE .../routes/{rid}`.
  Routes in a list are returned in match order (longest prefix first).
- [x] **A3 (M) Revisions:** `GET /api/revisions` (paged),
  `GET /api/revisions/{version}`, `POST /api/revisions/{version}/rollback`.
- [x] **A4 (S) Resolver and status:** `POST /api/resolve {host, path}` returns
  the matched subsystem, route, and the exact upstream URL it would forward to
  (or the reason there is no match), using the live snapshot.
  `GET /api/status` returns data plane listeners, snapshot version, uptime,
  DB path, and totals. The existing `/api/health` and `/api/config` (UI boot
  config) stay unchanged.
- [x] **A5 (S) Admin safety:** if `admin.listen` is not a loopback address, log
  a prominent warning that the admin plane has no authentication until M5.
  Loopback by default is the only protection in M1.

*Status for A1-A2:* done, with the response shape `{ data, version }` (deletes
return `{ version }`), camelCase JSON, and a `fields` array on 400s.
Subsystem resources carry `createdAtMs`/`updatedAtMs`. Unknown JSON keys are
rejected. A3 (revisions, rollback) and A4 (resolver, status) are done: the
revision list pages with `limit`/`before` and returns `nextBefore`; the
resolver reuses `build_upstream_uri`, which moved from the data plane into
`fast-proxy-routing` so both compute the identical URL; `GET /api/status`
reports the actually bound listeners. Request totals will join status with T1.

*Done for A1-A4:* every endpoint has HTTP-level tests in the style of
`src/routes/tests.rs` (success, validation error with `fields`, 404, 409), and
a test proves that each successful write is visible to the data plane without
restart.

### Workstream U: Admin UI

*Status: done.* How it is built: `lib/http.ts` (request core, `ApiError` with
per-field messages), `lib/types.ts`, `lib/api.ts` (one function per endpoint),
`lib/validation.ts` (pure client-side checks using the server's camelCase field
names, so client and server errors land on the same inputs),
`components/forms/` (one form per resource) and `components/subsystem/` (the
detail page's sections). Every form validates cheaply on the client and shows
the server's `fields` inline; errors that name no field (a 409, a 404) appear
once above the form; success and failure are toasts. Any write refreshes every
query except the boot config, and a `configChanged` push does the same for
changes made elsewhere. Test helpers: `test/mockApi.ts` (a recording fetch mock)
and `test/renderPage.tsx` (router, query client and real toasts). The UI could
not be looked at in a real browser in the session that built it; it is covered
by jsdom tests only, so a manual click-through is still worthwhile.

Follow existing component patterns and the tokens in
`ui/src/styles/index.css`; no hard-coded one-off colors.

- [x] **U1 (S) API client and types** in `ui/src/lib/api.ts` for all A1-A4
  endpoints; `ApiError` exposes `fields` for form display. Update
  `vite.config.ts` proxy target to the admin port.
- [x] **U2 (M) Navigation and shell:** sidebar items Dashboard, Subsystems,
  Revisions, Resolver, Settings; update `router.tsx`, `Sidebar.tsx`,
  `Topbar.tsx` titles.
- [x] **U3 (L) Subsystems list and detail.**
  - List: name, host chips, enabled toggle, route count, "New subsystem"
    button, empty state that explains the first steps.
  - Detail with sections: Hosts (add/remove), Upstreams (name + `host:port`),
    Routes (prefix, upstream, strip prefix, upstream prefix, preserve host,
    timeouts, enabled), shown in match order.
  - Forms validate on the client where cheap and always show server `fields`
    errors inline; success and failure toasts.
  - Delete confirmations that state what will be removed.
- [x] **U4 (M) Revisions page:** table of versions with summary and time,
  view-config modal, rollback button with confirmation.
- [x] **U5 (S) Resolver page:** host and path inputs, shows matched subsystem,
  route, and final upstream URL, or why nothing matched.
- [x] **U6 (M) Dashboard rewrite:** replace sample tiles with real data from the
  `traffic` event: global requests/s, error rate, active connections, a live
  chart, a per-subsystem table, and the activity feed showing config changes.
  Handle new event types in `LiveContext.tsx`.
- [x] **U7 (S) Tests and build:** Vitest tests for the forms (validation errors
  rendered, submit calls the right API), subsystem list empty state, and the
  rollback confirmation. `npm test` and `npm run build` pass.

### Workstream Q: Quality

- [~] **Q1 (L) Integration test harness** (`tests/` directory): helper that
  starts fake upstreams (in-process hyper servers on ephemeral ports with
  configurable behavior: echo headers, delay, close, WebSocket echo), a proxy
  instance with a temp state dir and ephemeral ports, and a small admin API
  client. Scenarios, each its own test:
  1. two subsystems, different hosts, different backends (the exit criteria)
  2. wildcard host and exact host precedence
  3. longest-prefix routing and segment boundary (`/api` vs `/apix`)
  4. strip prefix and upstream prefix rewriting, query preserved
  5. unknown host, no route, disabled subsystem, disabled route
  6. upstream down (502) and slow (504)
  7. config edit takes effect without restart
  8. restart keeps configuration (same DB reopened)
  9. rollback restores earlier behavior
  10. header handling (hop-by-hop, forwarded, request id)
  11. WebSocket passthrough
  12. graceful shutdown with an in-flight request

  *Status:* `tests/exit_criteria.rs` runs the real binary against in-process
  backends and covers scenarios 1, 5 (unknown host, disabled subsystem), 7 and
  8 (and 9, rollback); scenarios 2-4, 6 and 10 are covered at crate level in the data plane's
  forwarding tests. Scenarios 9 (rollback), 11 (WebSocket) and 12 (graceful shutdown) are covered too, at crate level and on the binary (12).
- [ ] **Q2 (M) Reload-under-load test:** N concurrent clients issue requests
  continuously while a task performs many admin writes (add/remove routes
  unrelated to the traffic and flip one route between two backends).
  Assert zero failed requests and that every response came from a backend
  that was valid at some point during the run.
- [ ] **Q3 (S) Baseline benchmark:** run `oha` or `wrk` against a local
  backend, direct versus through the proxy; record the numbers, hardware, and
  command in `docs/benchmarks.md` as the M1 baseline (informational, not a
  gate).
- [ ] **Q4 (S) Gates:** `cargo fmt --all -- --check`,
  `cargo clippy --all-targets -- -D warnings`, `cargo test`, `npm test`,
  `npm run build` all green in CI on Linux, macOS, and Windows. Fix any
  platform-specific issues (paths, signals) that appear.

### Workstream W: Documentation

- [ ] **W1 (M) README:** new quick start (start the proxy, open the admin UI,
  create a subsystem, curl through it), updated config table and CLI, API
  table, project structure, and removal of scaffold-only sections.
- [ ] **W2 (S) AGENTS.md:** new layout, where to add endpoints, snapshot and
  store rules (data plane never queries the DB; all writes go through
  `ConfigService`), test expectations.
- [ ] **W3 (S) ROADMAP.md:** tick M0 and M1 items, record decisions D1-D12 in
  the decision log, link this plan.
- [x] **W4 (S) Walkthrough:** `docs/getting-started.md` with a copy-paste curl
  example that sets up two subsystems through the admin API.

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| P2, P3 | P1 (decisions only for P1 items that affect later tasks; P2/P3 can start immediately) |
| C1-C3 | P3 |
| S1-S2 | P3, D1 resolved |
| S3 | none (pure code) |
| S4 | S1, S2, S3 |
| S5 | S4 |
| R1-R4 | S3 (types), can use hand-built config in tests |
| R5 | S4, R1, R4 |
| D1 | C1, P3 |
| D2-D5 | D1, R4 |
| D6 | D1 |
| D7 | C1, R5, D1, D2 |
| T1 | D2 |
| T2 | D2 |
| T3 | T1 |
| A1-A3 | R5, S5 (for A3) |
| A4 | R5 |
| A5 | C1 |
| U1-U2 | A1 (shapes can be agreed up front) |
| U3 | A1, A2 |
| U4 | A3 |
| U5 | A4 |
| U6 | T3 |
| Q1 | D7, A1, A2 |
| Q2 | Q1, R5 |
| W1-W4 | feature tasks complete |

Suggested slicing into pull requests, each leaving `main` green and releasable:

1. **PR1** P1, P2, P3 (groundwork and cleanup)
2. **PR2** C1-C3 (new config model, ports, docs for config)
3. **PR3** workspace conversion plus S1-S5 (model and store crates, validation, revisions) with tests
4. **PR4** R1-R5 (snapshot, matchers, apply pipeline) with tests
5. **PR5** D1-D4, D7 (data plane forwarding) plus Q1 harness and scenarios 1-8, 10
6. **PR6** D5, D6 (upgrade, graceful shutdown) plus scenarios 11-12
7. **PR7** T1-T3 (metrics, access log, live stats)
8. **PR8** A1-A5 (admin API) with tests
9. **PR9** U1-U7 (admin UI)
10. **PR10** Q2, Q3, Q4, W1-W4 (load test, benchmark, docs, polish)

PRs 3 and 4 can proceed in parallel after PR2; PR5 needs both. Admin UI work
(PR9) can start against the agreed API shapes once PR8 is in review.

## 8. Risks specific to M1

| Risk | Mitigation |
|---|---|
| Hyper client pool lacks a control we need (timeouts, per-host limits, cancel on client drop) | P1 spike before building D4; fall back to a thin custom pool over `hyper::client::conn` |
| Config file breaking change (`[server]` removed, ports moved) | Nothing is deployed; change directly, no compatibility shims (see AGENTS.md) |
| Streaming bodies accidentally buffered, hurting memory and latency | D4 tests with a multi-hundred-MB body and a memory ceiling check; code review item |
| Writer lock plus DB thread becomes a bottleneck for bulk admin edits | Not a data plane concern; measure in Q2, add a batch endpoint later if needed |
| Path handling edge cases (encoded slashes, `..`, double slashes) cause routing surprises or security issues | R3/S3 reject unsafe prefixes; D4 forwards the request path unmodified except for the documented prefix rules; add tests for encoded and dotted paths |
| Admin plane unauthenticated | Loopback default and warning (A5); authentication is the first M5 task |
| SQLite blocking or file corruption on abrupt exit | WAL mode, single writer thread, `synchronous=NORMAL`, restart test in Q1 |
| Scope creep into M2 (multi-endpoint, health) | D4 enforces one endpoint; new ideas go to the roadmap, not this plan |

## 9. Definition of done for M1

- [ ] Every task above is checked, or explicitly moved with a note.
- [ ] Exit criteria (section 1) demonstrated by integration test 1, 7, 8 and a
  manual run through the admin UI.
- [ ] CI green on all three platforms; no new clippy warnings.
- [ ] Overall test coverage above 60%, number recorded (AGENTS.md rule).
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [ ] README, AGENTS.md, ROADMAP.md, and getting-started updated.
- [ ] Baseline benchmark recorded.
- [ ] Release `v0.2.0` prepared through the existing release workflow.
