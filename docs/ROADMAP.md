# fast-proxy: Plan and Roadmap

Status: draft v0.1 (2026-11-09). This is a living document; update it as
decisions are made and milestones land.

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

## 2. Starting point

The repo is a single-binary fullstack scaffold:

| Piece | Reuse plan |
|---|---|
| Axum + Tokio + Clap + Serde + Tracing | Keep. Axum serves the **admin plane only**. |
| `AppError` JSON envelope | Keep for the admin API. |
| TOML config + CLI overrides (`config.rs`) | Extend into the proxy config model. |
| Access log middleware (`access_log.rs`) | Move to the data plane, extend with route/upstream fields. |
| Embedded React UI + WebSocket push | Becomes the admin UI (routes, upstreams, certs, live traffic). |
| Metrics / events services | Replace sample data with real proxy metrics. |
| Tasks service, component gallery, `/api/error-demo` | Remove (M0). |
| CI + two-step release workflow | Keep. |

## 3. Architecture

### 3.1 Two planes, two listeners

- **Data plane**: public listeners (HTTP and HTTPS) built directly on
  **hyper** + Tokio. No Axum in the hot path.
- **Admin plane**: Axum REST + WebSocket + embedded UI on a separate listener,
  bound to `127.0.0.1` by default. Never reachable through the data plane
  unless an operator explicitly routes to it.

Separate listeners keep admin failures, auth, and load from affecting traffic.

### 3.2 Config as an immutable snapshot

- Config is parsed and **validated into a compiled snapshot** (routing tables,
  upstream groups, TLS cert resolver) and published through `arc-swap`.
- Each request loads the current snapshot once and uses it for its entire
  lifetime. In-flight requests finish on the old snapshot.
- Updates (file watch, admin API, SIGHUP) follow **validate -> compile -> swap**.
  An invalid config is rejected and the old snapshot stays live.
- Every snapshot has a monotonically increasing version and is persisted, so
  rollback is "re-apply version N".
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

### 3.4 Key technology choices (proposed)

| Concern | Choice | Notes |
|---|---|---|
| HTTP | `hyper` 1.x + `hyper-util` | HTTP/1.1 and HTTP/2, upgrade (WebSocket) passthrough |
| TLS | `rustls` + `tokio-rustls` | Custom `ResolvesServerCert` for SNI + hot reload |
| ACME | `instant-acme` (evaluate vs `rustls-acme`) | HTTP-01 first, TLS-ALPN-01 later |
| Config swap | `arc-swap` | Lock-free reads on the hot path |
| Host/prefix match | `HashMap` + wildcard suffix map; radix tree for prefixes | Benchmark before adopting a crate |
| Metrics | `prometheus` or `metrics` + exporter | Decide in M1 |
| Upstream client | hyper-util legacy client or custom pool | Decide in M1 based on pool control needs |

### 3.5 Proposed config shape (illustrative, not final)

```toml
[admin]
listen = "127.0.0.1:9000"

[[listener]]
name = "http"
listen = "0.0.0.0:80"

[[listener]]
name = "https"
listen = "0.0.0.0:443"
tls = true

[[upstream]]
name = "api"
lb = "p2c"                      # round_robin | weighted | least_conn | p2c | hash
[[upstream.endpoint]]
addr = "10.0.0.11:8080"
weight = 1
[upstream.health]
path = "/healthz"
interval = "5s"
unhealthy_threshold = 3
healthy_threshold = 2

[[host]]
names = ["example.com", "*.example.com"]
tls = { acme = true }
[[host.route]]
prefix = "/api"
upstream = "api"
strip_prefix = true
[[host.route]]
prefix = "/"
upstream = "web"
```

## 4. Feature design notes

### 4.1 Host and prefix routing (M1)

- Match order: exact host, longest wildcard suffix (`*.example.com`), default
  host. Port is ignored unless configured. Host is lowercased and trailing dot
  stripped. TLS connections cross-check SNI against `Host`.
- Within a host: longest-prefix wins, matched on path-segment boundaries
  (`/api` matches `/api/x`, not `/apix`). Optional strip/rewrite of prefix.
- Unknown host returns a configurable 404 (or 421 for SNI/Host mismatch).
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

- SNI-based certificate selection via a custom resolver backed by the config
  snapshot, so cert changes apply without restart.
- Static certs from files with file-watch reload; minimum TLS 1.2, sane cipher
  defaults, ALPN for `h2` and `http/1.1`.
- HTTP -> HTTPS redirect option per host. HSTS as an opt-in header.
- Optional upstream TLS (re-encrypt) with CA bundle and SNI override.

### 4.4 ACME (M4)

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

- Scope: global, per host, or per route. Toggle via admin API, UI, or CLI.
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

### M0: Rebrand and cleanup

- [x] Rename project to `fast-proxy`; update description, README, config, CI.
- [x] Plan and roadmap document (this file).
- [ ] Remove scaffold demos (tasks service, component gallery, error-demo).
- [ ] Split `main.rs` wiring into admin-plane and data-plane modules.
- [ ] Decide metrics library and upstream client approach (ADR notes in `docs/`).

### M1: Core proxy and realtime config

- [ ] Config model: listeners, hosts, routes, upstreams (single endpoint).
- [ ] Hyper-based data plane with plain HTTP listener; streaming bodies.
- [ ] Host routing (exact, wildcard, default) and prefix routing with strip.
- [ ] Forwarded headers, hop-by-hop handling, request IDs, timeouts.
- [ ] Immutable snapshot + `arc-swap`; validate-then-swap reload via SIGHUP,
      file watch, and admin API; config version history and rollback.
- [ ] Upstream connection pooling; WebSocket/upgrade passthrough.
- [ ] Graceful shutdown and connection draining.
- [ ] Baseline Prometheus metrics and enriched access log.

**Exit criteria:** an operator can route two hostnames with prefix rules to
different backends, edit the config live, and see correct metrics, with no
dropped in-flight requests across reloads.

### M2: Upstream groups, load balancing, health

- [ ] Multi-endpoint upstream groups with weights.
- [ ] Round robin, weighted, least-connections, P2C+EWMA, consistent hash.
- [ ] Active HTTP/TCP health checks; passive outlier ejection.
- [ ] Endpoint draining and manual disable via admin API.
- [ ] Idempotent retries with budget; all-unhealthy policy.
- [ ] Health and LB state in metrics and admin UI.

**Exit criteria:** killing one of three backends causes no sustained errors
and the endpoint returns to rotation automatically after recovery.

### M3: TLS termination

- [ ] HTTPS listener with rustls, SNI cert resolver, HTTP/2 via ALPN.
- [ ] Static cert loading with hot reload; self-signed dev fallback.
- [ ] HTTP -> HTTPS redirect, opt-in HSTS, TLS version policy.
- [ ] Optional TLS to upstreams.
- [ ] Cert expiry metrics.

### M4: ACME automatic certificates

- [ ] ACME client, account and cert persistence in the state directory.
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

### Initial performance targets (to be validated in M1/M7)

- Proxy overhead per request small relative to upstream latency (target: well
  under 1 ms added at p99 on a local benchmark).
- Throughput scales near-linearly with cores for small requests.
- No allocation or lock contention on the hot path beyond the snapshot load.
- Config reload with thousands of routes completes in milliseconds and does not
  affect in-flight request latency.

## 7. Risks and open questions

| Risk / question | Mitigation or next step |
|---|---|
| hyper client pool may not give enough control (per-endpoint limits, health-aware reuse) | Prototype in M1; fall back to a custom pool |
| ACME rate limits and failure modes can lock out a domain | Staging by default, persisted state, backoff, clear status in UI |
| Admin plane exposure is a high-impact security risk | Loopback default, auth in M5, audit log, never routable by default |
| Sampling can leak secrets or hurt latency | Redaction on by default, bodies opt-in, off the request path, bounded buffers |
| Windows support in CI vs. Unix-only features (signals for reload) | Provide admin API/CLI reload as the portable path |
| Where does config live: file only, or file + admin API writes? | Proposal: file is the source of truth; API edits write back to it (or a managed overlay). Decide in M1 |
| Metrics crate choice and label cardinality | Decide in M0 ADR |

## 8. Decision log

| Date | Decision |
|---|---|
| 2026-11-09 | Project renamed `fast-proxy`; goal set to ingress/reverse proxy. |
| 2026-11-09 | Proposed: hyper data plane, Axum admin plane, `arc-swap` snapshots, rustls, tail-based sampling. Pending confirmation in M0/M1. |
