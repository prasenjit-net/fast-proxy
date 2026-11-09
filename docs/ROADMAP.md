# fast-proxy: Plan and Roadmap

Status: v0.3 (2026-11-09). **M0 and M1 are complete**; M2 is next. This is a
living document; update it as decisions are made and milestones land.

## 1. Vision

A fast ingress server and reverse proxy that ships as **one binary**, is
**reconfigurable at runtime without dropping a connection**, and makes its own
behavior observable. One process should replace the common "nginx + certbot +
health-check scripts + dashboards" stack for small and mid-size deployments.

### Goals

1. Host-header based virtual hosting.
2. URI prefix based routing.
3. TLS termination.
4. ACME based automatic certificates.
5. Diagnostics and monitoring.
6. Temporary, reversible maintenance mode.
7. Health monitoring of upstream server groups.
8. Load balancing across upstream groups.
9. Sampling of unusual requests for later inspection.

### Non-goals (for 1.0)

- Being a full API gateway (auth plugins, transformation DSLs, WAF).
- Service-mesh features (mTLS identity, xDS control plane).
- HTTP/3 / QUIC (revisit after 1.0).
- Clustering / distributed config consensus. A single node is the unit; config
  can be pushed to many nodes by external tooling.
- Layer-4 (raw TCP/UDP) proxying. Revisit after 1.0.

## 1.1 Core concepts

- **Data plane**: the public-facing proxy process. Its own configuration is
  small, static, and comes from the **config file**: listeners, TLS, ACME,
  admin listener address, DB path, logging. Changing it requires a restart.
- **Subsystem**: one per host name (or small set of host names). Each subsystem
  behaves like a separate, independently configurable proxy: its own routes,
  upstream groups, health checks, maintenance flag, sampling rules, and
  limits. A request is first matched to a subsystem by host name, and only
  then routed inside it.
- **Route**: belongs to exactly one subsystem. Matches by URI prefix within
  that subsystem and points to an upstream group.
- **Admin plane**: a separate listener with the admin UI and API. After the
  data plane is started from its config file, operators create and edit
  subsystems and routes here. They are stored in a **local database file**
  and applied live.

### Configuration ownership

| Layer | Source | Changes apply | Examples |
|---|---|---|---|
| Data plane | Config file (`config.toml`) | On restart | Listeners and ports, TLS/static certs, ACME account and directory, admin listener, DB path, state dir, logging |
| Subsystems and below | Local DB, edited via admin UI/API | Live, no restart | Subsystems, host names, routes, upstream groups, health checks, maintenance mode, sampling rules |

The config file never contains subsystems or routes. The DB never contains
listener, TLS, or ACME settings. This keeps "how the proxy is hosted" separate
from "what the proxy serves".

## 2. Where the project started

The repo began as a single-binary fullstack scaffold. What became of each piece:

| Piece | What happened |
|---|---|
| Axum + Tokio + Clap + Serde + Tracing | Kept. Axum serves the **admin plane only** (`fast-proxy-admin`). |
| `AppError` JSON envelope | Kept for the admin API; gained `Conflict` and `Validation` (with a `fields` array). |
| TOML config + CLI overrides (`config.rs`) | Extended into the process config model (`src/config/`, `src/cli.rs`). |
| Access log middleware (`access_log.rs`) | Became `AccessLog` in `fast-proxy-telemetry`, used by both planes, with data plane fields. |
| Embedded React UI + WebSocket push | Became the admin UI (subsystems, revisions, resolver, live dashboard). |
| Metrics / events services | The fake CPU/memory sampler was replaced by real Prometheus metrics and traffic stats. |
| Tasks service, component gallery, `/api/error-demo` | Removed (M0). |
| CI + two-step release workflow | Kept; CI now also runs the end-to-end tests on three platforms. |

## 3. Architecture

### 3.1 Two planes, two listeners

- **Data plane**: public listeners (HTTP and HTTPS) built directly on
  **hyper** + Tokio. No Axum in the hot path.
- **Admin plane**: Axum REST + WebSocket + embedded UI on a separate listener,
  bound to `127.0.0.1` by default. Never reachable through the data plane
  unless an operator explicitly routes to it.

Separate listeners keep admin failures, auth, and load from affecting traffic.

### 3.2 Config as an immutable snapshot

- At startup the data plane reads the config file, opens the DB, and
  **compiles the DB contents into a snapshot** (host -> subsystem table, per
  subsystem route table and upstream groups) published through `arc-swap`.
- Each request loads the current snapshot once and uses it for its entire
  lifetime. In-flight requests finish on the old snapshot.
- Admin writes follow **validate -> commit to DB (transaction) -> recompile
  -> swap**. An invalid change is rejected before commit and the old snapshot
  stays live. Recompiling can be per subsystem so edits to one subsystem do not
  rebuild the others.
- The DB is the source of truth for subsystems and below. Every committed
  change bumps a version and writes a history row, so rollback is "re-apply
  version N". There is no file-watch reload for subsystem config; only the
  data plane file is read, and only at startup.
- Runtime state that must survive swaps (health status, connection counts,
  maintenance flags, EWMA latency) lives in separate shared structures keyed by
  stable IDs, not inside the snapshot.

### 3.3 Request path

```
accept -> (TLS handshake, SNI cert pick) -> parse HTTP
  -> load snapshot
  -> host match (exact, then wildcard, then default)
  -> prefix match (longest prefix within host)
  -> maintenance check (host/route flag, bypass rules)
  -> upstream group -> LB picks healthy endpoint
  -> forward (pooled connection, timeouts, retries on idempotent failures)
  -> stream response back
  -> record metrics + access log + sampling decision
```

### 3.4 Key technology choices

| Concern | Choice | Notes |
|---|---|---|
| HTTP | `hyper` 1.x + `hyper-util` | HTTP/1.1 and upgrade (WebSocket) passthrough done in M1; HTTP/2 arrives with TLS (M3) |
| TLS | `rustls` + `tokio-rustls` | Custom `ResolvesServerCert` for SNI + hot reload |
| ACME | `instant-acme` (evaluate vs `rustls-acme`) | HTTP-01 first, TLS-ALPN-01 later |
| Config swap | `arc-swap` | Lock-free reads on the hot path (done in M1) |
| Host/prefix match | `HashMap` + wildcard suffix map; segment-aware longest-prefix lookup | **Decided (M1):** own implementation, property-tested against a brute-force reference ([ADR 0004](adr/0004-own-host-and-route-matchers.md)). Revisit if the benchmark shows routing as a bottleneck |
| Metrics | `prometheus` crate | **Decided (M1):** behind our own `Metrics` type so it can be swapped ([ADR 0003](adr/0003-prometheus-crate-for-metrics.md)) |
| Upstream client | hyper-util legacy client | **Decided (M1):** a spike confirmed pooling, per-request timeouts, streaming and cancellation; one pool per distinct connect timeout ([ADR 0002](adr/0002-hyper-for-the-data-plane.md)) |

### 3.5 Data plane config file (illustrative, not final)

```toml
[data_plane]
state_dir = "./state"           # ACME account/certs, runtime files
database  = "./state/fast-proxy.db"

[[data_plane.listener]]
name = "http"
listen = "0.0.0.0:80"

[[data_plane.listener]]
name = "https"
listen = "0.0.0.0:443"
tls = true

# Static certificates, selected by SNI pattern.
[[data_plane.tls.cert]]
names = ["internal.example.com"]
cert_file = "/etc/fast-proxy/internal.pem"
key_file  = "/etc/fast-proxy/internal.key"

# ACME: account and directory only. Which host names get certs is derived
# from the subsystems in the DB (see 4.4).
[data_plane.acme]
enabled = true
email = "ops@example.com"
directory = "https://acme-staging-v02.api.letsencrypt.org/directory"
allowed_domains = ["*.example.com", "example.org"]   # safety allowlist

[admin]
listen = "127.0.0.1:9000"

[logging]
level = "info"
access_log = "access.log"
```

*As built in M1:* `state_dir`, `database`, `trusted_proxies`, `shutdown_grace`,
`[[data_plane.listener]]` (`name`, `listen`), `[admin].listen` and `[logging]`
exist, and unknown keys (including `tls`) are rejected. The TLS and ACME
sections above are still illustrative and arrive in M3 and M4.

### 3.6 Subsystem model stored in the DB (illustrative, not final)

```
subsystem        id, name, enabled, maintenance (state, ttl, message), created/updated
subsystem_host   subsystem_id, host_pattern (exact | *.wildcard), unique across all subsystems
upstream_group   id, subsystem_id, name, lb_algorithm, health settings
upstream_endpoint id, group_id, addr, weight, state (enabled/disabled/draining)
route            id, subsystem_id, prefix, upstream_group_id, strip_prefix, rewrite,
                 timeouts, retries, priority, enabled
sampling_rule    id, subsystem_id, triggers, limits
config_history   version, timestamp, actor, change summary, snapshot/diff
```

Rules enforced on write: a host pattern belongs to at most one subsystem,
route prefixes are unique within a subsystem, and a route's upstream group must
belong to the same subsystem (no cross-subsystem references, so subsystems stay
isolated).

*As built in M1:* the tables are `subsystem` (with `description`),
`subsystem_host`, `upstream_group`, `upstream_endpoint` (one per group for now),
`route` and `revision` (the full configuration as JSON per change, the last 100
kept, in place of `config_history`). Maintenance, load-balancing and sampling
columns arrive with their milestones; there are no migrations while the project
is undeployed (the schema file is edited in place).

Storage: **SQLite** via `rusqlite` in WAL mode, a single local
file. Chosen for transactions, schema migrations, and easy inspection. Access
goes through a repository layer so the backend can be swapped later (for
example a remote DB for multi-node setups) without touching the proxy.

## 4. Feature design notes

### 4.1 Subsystems, host and prefix routing (M1)

- **Step 1, subsystem selection**: match the request host against all
  subsystem host patterns: exact host first, then longest wildcard suffix
  (`*.example.com`). Port is ignored unless configured. Host is lowercased and
  trailing dot stripped. TLS connections cross-check SNI against `Host`.
- **Step 2, route selection inside the subsystem**: longest-prefix wins,
  matched on path-segment boundaries (`/api` matches `/api/x`, not `/apix`).
  Optional strip/rewrite of prefix.
- Subsystem state (enabled, maintenance, health, limits, metrics) is
  independent: disabling or breaking one subsystem never affects another.
- Unknown host (no subsystem) returns a data-plane-level 404 (or 421 for
  SNI/Host mismatch). A subsystem with no matching route returns that
  subsystem's own 404. Both responses are configurable.
- Forwarded headers: `X-Forwarded-For/Proto/Host` and `Forwarded`, with a
  trusted-proxy list so client-supplied values are not blindly trusted.
- Hop-by-hop header stripping per RFC 9110.

### 4.2 Load balancing and health (M2)

- Algorithms: round robin, weighted round robin, least connections,
  power-of-two-choices with EWMA latency, and consistent hash (by client IP,
  header, or cookie) for affinity.
- **Active health checks**: HTTP (path, expected status) and TCP connect, with
  interval, timeout, and rise/fall thresholds.
- **Passive health checks**: eject an endpoint after N consecutive failures or
  an elevated error rate, with exponential backoff before re-admission.
- Endpoint states: `healthy`, `unhealthy`, `draining`, `disabled` (manual).
  Draining stops new requests but lets in-flight ones finish.
- Retries only for idempotent methods and connect-phase failures by default,
  with a per-route retry budget to avoid retry storms.
- If all endpoints are unhealthy: configurable fail-open ("panic mode") or 503.

### 4.3 TLS termination (M3)

- TLS is **data plane config**: listeners, static certs, and TLS policy come
  from the config file.
- SNI-based certificate selection via a custom resolver. It consults static
  certs from the config file and ACME-managed certs, so renewed certs apply
  without restart.
- Static certs from files with file-watch reload; minimum TLS 1.2, sane cipher
  defaults, ALPN for `h2` and `http/1.1`.
- HTTP -> HTTPS redirect and HSTS are per-subsystem settings (DB, live).
- Optional upstream TLS (re-encrypt) with CA bundle and SNI override.

### 4.4 ACME (M4)

- ACME is **data plane config** (account email, directory, state dir,
  allowlist) in the config file. The set of host names to certify is derived
  from the subsystem host patterns in the DB: adding a subsystem host in the
  admin UI triggers issuance, if it matches `allowed_domains`. Wildcard host
  patterns need DNS-01 and are not auto-issued until that lands.
- Account key and issued certs stored in a configurable state directory with
  restrictive permissions.
- HTTP-01 challenge served on the plain HTTP listener ahead of routing.
  TLS-ALPN-01 and DNS-01 (wildcards) are later candidates.
- Renewal loop: renew at ~2/3 of lifetime, jittered, with backoff on failure and
  a visible status per domain (admin API and UI).
- Use the **Let's Encrypt staging** directory by default in dev; guard against
  rate limits by caching and never re-ordering on every restart.
- Serve a self-signed fallback cert while the first issuance is pending.

### 4.5 Maintenance mode (M5)

- Scope: global, per subsystem, or per route. Toggle via admin API, UI, or
  CLI. Stored in the DB and applied live.
- Behavior: respond 503 with `Retry-After`, a configurable HTML or JSON body,
  and no upstream traffic.
- **Temporary by design**: a toggle takes an optional TTL and auto-expires.
- Bypass rules: allowlisted source CIDRs or a secret header, so operators can
  verify a deploy before reopening.
- State is persisted so a restart during maintenance stays in maintenance.
- Health checks continue during maintenance so the group is ready on exit.

### 4.6 Diagnostics and monitoring (grown across M1-M5)

- **Metrics** (Prometheus format on the admin listener): requests, status
  classes, latency histograms, bytes, active connections, all labeled by
  host/route/upstream; upstream health state; TLS handshakes; cert expiry;
  ACME status; config version and reload outcomes. Keep label cardinality
  bounded (labels come from config, never raw paths).
- **Logs**: structured access log (route, upstream, endpoint, timings, status,
  bytes, request ID) and a separate `tracing` diagnostic log.
- **Request IDs**: generated or propagated (`X-Request-Id`) and forwarded.
- **Admin UI/API**: live traffic, route and upstream tables with health,
  cert status, config version history, and current maintenance flags.
- Readiness and liveness endpoints for the proxy itself.

### 4.7 Request sampling of unusual requests (M6)

Sampling is **tail-based**: the decision is made after the response, so the
interesting cases can be selected.

- Triggers: 5xx, upstream timeouts/connect errors, latency above a threshold
  (static or relative to a route's rolling p99), 4xx spikes per client,
  oversized requests/responses, malformed or unusual headers, retried requests,
  plus a small baseline random sample for comparison.
- Captured: method, path, selected headers, status, timings, upstream, trigger
  reason, and optionally a size-capped body.
- **Safety**: header and body redaction (Authorization, Cookie, configurable
  list), body capture off by default, per-route opt-in.
- **Bounded cost**: in-memory ring buffer with a global rate limit, optional
  spill to a rotating file. Sampling must never block or slow the request path.
- Browsable and exportable from the admin UI and API.

## 5. Roadmap

Milestones are ordered by dependency, not by calendar. Each ends with a
working, releasable binary.

### M0: Rebrand and cleanup (complete)

- [x] Rename project to `fast-proxy`; update description, README, config, CI.
- [x] Plan and roadmap document (this file).
- [x] Remove scaffold demos (tasks service, component gallery, error-demo).
- [x] Split `main.rs` wiring into admin-plane and data-plane modules (now a
      Cargo workspace of small crates).
- [x] Decide metrics library and upstream client approach (ADR notes in `docs/adr/`).

### M1: Core proxy and realtime config (complete)

Detailed task plan: [M1-PLAN.md](M1-PLAN.md).

- [x] Data plane config file model: listeners, DB path, state dir, admin
      listener, logging (TLS and ACME sections added in M3/M4).
- [x] SQLite storage layer: schema, migrations, repository API for
      subsystems, hosts, routes, upstreams, and config history.
- [x] Hyper-based data plane with plain HTTP listener; streaming bodies.
- [x] Subsystem selection by host (exact, wildcard) and prefix routing inside
      the subsystem with strip; unknown host and no-route responses.
- [x] Forwarded headers, hop-by-hop handling, request IDs, timeouts.
- [x] Immutable snapshot + `arc-swap`; admin write -> validate -> DB commit ->
      recompile -> swap; config version history and rollback.
- [x] Admin API (CRUD for subsystems, hosts, routes, single-endpoint
      upstreams) and a first admin UI to manage them. Start the proxy from the
      config file with an empty DB, then configure everything from the UI.
- [x] Upstream connection pooling; WebSocket/upgrade passthrough.
- [x] Graceful shutdown and connection draining.
- [x] Baseline Prometheus metrics and enriched access log.

**Exit criteria:** starting from an empty DB, an operator creates two
subsystems (two host names) in the admin UI, adds prefix routes to different
backends, and traffic flows immediately. Edits apply live with correct metrics
and no dropped in-flight requests, and survive a restart.

*Demonstrated by* `tests/exit_criteria.rs` (empty database, two subsystems routed
immediately, live edits, rollback, restart, graceful shutdown) and
`tests/reload_under_load.rs` (zero failed requests and no stale routing during
400 live writes). The one thing not exercised is a person clicking through the
UI in a real browser; the UI is covered by jsdom tests. What M1 left for later is
listed in [M1-PLAN.md](M1-PLAN.md) section 10.

### M2: Upstream groups, load balancing, health

Detailed task plan: [M2-PLAN.md](M2-PLAN.md) (draft; decisions awaiting confirmation).

- [ ] Multi-endpoint upstream groups with weights.
- [ ] Round robin, weighted, least-connections, P2C+EWMA, consistent hash.
- [ ] Active HTTP/TCP health checks; passive outlier ejection.
- [ ] Endpoint draining and manual disable via admin API.
- [ ] Idempotent retries with budget; all-unhealthy policy.
- [ ] Health and LB state in metrics and admin UI.

**Exit criteria:** killing one of three backends causes no sustained errors
and the endpoint returns to rotation automatically after recovery.

### M3: TLS termination

- [ ] `[data_plane.tls]` config section; HTTPS listener with rustls, SNI cert
      resolver, HTTP/2 via ALPN.
- [ ] Static cert loading with hot reload; self-signed dev fallback.
- [ ] TLS version policy (config file); per-subsystem HTTP -> HTTPS redirect
      and opt-in HSTS (DB).
- [ ] Optional TLS to upstreams.
- [ ] Cert expiry metrics.

### M4: ACME automatic certificates

- [ ] `[data_plane.acme]` config section; ACME client, account and cert
      persistence in the state directory.
- [ ] Derive domains to certify from subsystem hosts in the DB, filtered by
      the config allowlist; react to host add/remove at runtime.
- [ ] HTTP-01 challenge handling ahead of routing.
- [ ] Renewal scheduler with jitter, backoff, and status reporting.
- [ ] Staging/production directory switch; rate-limit safeguards.
- [ ] Stretch: TLS-ALPN-01, DNS-01 for wildcards.

### M5: Operations control and diagnostics UI

- [ ] Maintenance mode (global/host/route) with TTL, bypass rules, persistence.
- [ ] Admin auth (token or basic auth to start) and audit log of changes.
- [ ] Admin UI pages: routes, upstreams and health, certs, maintenance,
      config history, live traffic.
- [ ] CLI subcommands for common operations (`maintenance on/off`, `reload`,
      `config validate`).

### M6: Request sampling

- [ ] Tail-based sampler with trigger rules and baseline sampling.
- [ ] Redaction, size caps, per-route opt-in for bodies.
- [ ] Bounded ring buffer + optional file spill; global rate limit.
- [ ] Admin API and UI to browse, filter, and export samples.

### M7: Hardening and 1.0

- [ ] Rate limiting and connection limits; slowloris and header-size defenses.
- [ ] Load and soak testing; published benchmarks; profile-driven tuning.
- [ ] Fuzzing of the request parser path and config loader.
- [ ] Security review (admin plane exposure, ACME key storage, header trust).
- [ ] Docs: operator guide, config reference, deployment recipes
      (systemd, Docker).
- [ ] Packaging: container image, release binaries (already in CI).

### Post-1.0 candidates

HTTP/3, TCP/UDP proxying, DNS-01 providers, service discovery (DNS SRV,
Consul, Kubernetes), gRPC-aware features, response caching, mirroring/shadow
traffic, multi-node config sync.

## 6. Testing and quality strategy

- **Unit tests** for matchers, LB algorithms, config validation, and
  redaction.
- **Integration tests** that spawn the proxy and in-process fake upstreams:
  routing, retries, failover, maintenance, reload under load, WebSocket.
- **TLS/ACME tests** against a local ACME test server (e.g. Pebble) in CI.
- **Property and fuzz tests** for host/prefix matching and config parsing.
- **Benchmarks** (criterion for components; `wrk`/`oha` end to end) tracked
  across releases to catch regressions.
- Existing gates stay: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`,
  frontend tests and build, on Linux/macOS/Windows.

*After M1:* unit tests per crate, property tests for the matchers, end-to-end
tests that run the binary against in-process backends (including reload under
load and WebSocket passthrough), UI tests, and an `oha` baseline
([benchmarks.md](benchmarks.md)) exist. Not yet: TLS/ACME tests (M3/M4), fuzzing
(M7), and a Windows end-to-end test of the console Ctrl-C path (the signal tests
are Unix-only).

### Initial performance targets (to be validated in M1/M7)

- Proxy overhead per request small relative to upstream latency (target: well
  under 1 ms added at p99 on a local benchmark).
- Throughput scales near-linearly with cores for small requests.
- No allocation or lock contention on the hot path beyond the snapshot load.
- Config reload with thousands of routes completes in milliseconds and does not
  affect in-flight request latency.

*M1 baseline* ([benchmarks.md](benchmarks.md)): about 25 us added per request at
p50 (and p99) on one connection over loopback, so the "well under 1 ms" target
holds; under saturation on a shared CPU the proxy sustains a bit under half of
direct throughput. Reload with thousands of routes is not measured yet.

## 7. Risks and open questions

| Risk / question | Mitigation or next step |
|---|---|
| hyper client pool may not give enough control (per-endpoint limits, health-aware reuse) | **Resolved for M1:** the spike passed ([ADR 0002](adr/0002-hyper-for-the-data-plane.md)). Re-check in M2 when per-endpoint limits and health-aware reuse matter |
| ACME rate limits and failure modes can lock out a domain | Staging by default, persisted state, backoff, clear status in UI |
| Admin plane exposure is a high-impact security risk | Loopback default, auth in M5, audit log, never routable by default |
| Sampling can leak secrets or hurt latency | Redaction on by default, bodies opt-in, off the request path, bounded buffers |
| Windows support in CI vs. Unix-only features (signals for reload) | **Seen in M1:** the SIGINT end-to-end tests could not run on Windows and are Unix-only. Draining is also tested without signals. Reload is via the admin API, which is portable |
| ACME needs host names that live in the DB, but ACME config lives in the file | Derive domains from DB hosts, constrained by a file allowlist; wildcards wait for DNS-01 |
| SQLite from async code (blocking I/O) | **Done in M1:** one dedicated store thread; the hot path only reads the in-memory snapshot, never the DB |
| Admin UI is the only way to configure subsystems; no config-as-code | Add import/export (JSON/TOML) of subsystem config in M5 so setups are reproducible and reviewable |
| Backup and portability of the DB file | Document file location; add export and a consistent online backup command |
| Metrics crate choice and label cardinality | **Done in M1:** `prometheus` crate; labels come only from config names, enforced by tests. Per-request label-lookup cost is not measured separately yet |

## 8. Decision log

| Date | Decision |
|---|---|
| 2026-11-09 | Project renamed `fast-proxy`; goal set to ingress/reverse proxy. |
| 2026-11-09 | Each host name is a **subsystem**: independent, separately configurable, selected by host first, then routed by URI prefix within it. |
| 2026-11-09 | Config split: data plane (listeners, TLS, ACME, admin address, DB path) from the config file; subsystems, routes and upstreams from a local DB file, edited through the admin UI/API. Supersedes the earlier open question about file vs API as source of truth. |
| 2026-11-09 | **SQLite** via `rusqlite` (bundled, WAL) for the local DB, owner-confirmed ([ADR 0001](adr/0001-sqlite-via-rusqlite.md)). No migrations while undeployed; one dedicated store thread; the data plane never reads the DB. |
| 2026-11-09 | **hyper** data plane (ADR 0002, spike passed), Axum admin plane, `arc-swap` snapshots. rustls and tail-based sampling remain proposals for M3 and M6. |
| 2026-11-09 | M1 design decisions D1-D12 (see [M1-PLAN.md](M1-PLAN.md) section 3): D1 SQLite via rusqlite; D2 no migrations (one embedded schema, `user_version` check); D3 a dedicated DB thread; D4 exactly one endpoint per upstream group until M2; D5 hyper + hyper-util; D6 the `prometheus` crate (owner-confirmed); D7 our own host and route matchers; D8 integer ids with unique names used as metric labels; D9 last write wins; D10 do not trust incoming `X-Forwarded-*` unless the peer is in `trusted_proxies`; D11 the Host header is preserved unless a route rewrites it; D12 each change stores the full configuration, last 100 kept, rollback re-applies a revision as a new one. |
| 2026-11-09 | Writes go through one `ConfigService` in its own crate: mutate in a transaction, compile the would-be configuration as the validation, commit with a revision, swap the snapshot. Keeps `routing` free of the store and `dataplane` free of both. |
| 2026-11-09 | The upstream URL rewrite (`build_upstream_uri`) lives in `fast-proxy-routing`, shared by the data plane and the admin resolver so they cannot disagree. |
| 2026-11-09 | Metric labels come only from config names (subsystem name, route prefix; `_unmatched`/`_none` otherwise), never raw hosts or paths. Requests are recorded when the response body finishes, including when the client leaves mid-response. |
| 2026-11-09 | `route.upstream_group_id` uses NO ACTION rather than RESTRICT in the schema, so deleting a subsystem cascades while deleting a used upstream is still refused. |
| 2026-11-09 | The SIGINT end-to-end tests are Unix-only (they deliver a real signal); graceful draining is covered on every platform by signal-free tests. |
| 2026-11-09 | Performance is measured with `oha` against a small fixed-response backend example; the baseline is informational, not a gate ([benchmarks.md](benchmarks.md)). |
| 2026-11-09 | M2 design decisions E1-E16 (see [M2-PLAN.md](M2-PLAN.md) section 3 and ADRs [0005](adr/0005-runtime-state-and-endpoint-identity.md), [0006](adr/0006-selection-algorithms-and-eligibility.md), [0007](adr/0007-health-checks-and-ejection.md), [0008](adr/0008-retries-budget-and-body-replay.md)): stable endpoint ids; runtime state in carried `Arc<EndpointRuntime>`; new crates `balancing` and `health`; round robin default; fail-open default; passive ejection on, active checks off; retry connect failures and idempotent 5xx with a 20% budget; E12 body replay provisional pending spike G2. |
