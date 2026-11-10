# M2 Plan: Upstream groups, load balancing, health

Status: **v0.2 (2026-11-09), in progress.** The **(confirm)** items were accepted as recommended
(ADRs 0005-0008; E12 stays provisional until spike G2). Parent
document: [ROADMAP.md](ROADMAP.md). Predecessor: [M1-PLAN.md](M1-PLAN.md)
(complete; section 10 there lists what carries over). Task sizes are relative
effort (S under half a day, M about 1-2 days, L about 3-5 days), not
commitments. Tick the checkboxes as tasks land.

## 1. Goal and exit criteria

M1 sends each route to exactly one backend address. M2 makes an **upstream
group** a real thing: several endpoints behind one name, traffic spread across
them by a chosen algorithm, unhealthy endpoints taken out of rotation
automatically (and put back when they recover), operators able to drain or
disable an endpoint by hand, and failed requests retried safely.

**Exit criteria** (from the roadmap, made measurable):

1. An operator creates an upstream group with three endpoints in the admin UI
   and routes to it; traffic is spread across the endpoints according to the
   group's algorithm (within a stated tolerance in tests).
2. **Killing one of the three backends under load causes no sustained errors.**
   The endpoint is taken out of rotation within a bounded time (passive: after
   the configured number of consecutive failures; active: after `fall` failed
   probes), and requests that fail on it for connect-phase reasons are retried
   on another endpoint, so clients see no errors apart from requests that were
   already in flight on the dead connection.
3. **The endpoint returns to rotation automatically after recovery** (active:
   after `rise` successful probes; passive: after its ejection expires and a
   trial request succeeds), with no operator action.
4. An operator can **drain** or **disable** an endpoint from the admin UI/API: it
   receives no new requests, requests already in flight finish, and the state
   survives a restart.
5. When **every** endpoint is unhealthy, the group does what its policy says:
   fail open (keep serving, "panic mode") or answer 503.
6. All of it is observable: Prometheus metrics, the access log (which endpoint
   served the request, how many attempts), live endpoint state in the UI, and
   push events when an endpoint changes state.
7. **No regression:** the M1 exit criteria and `tests/reload_under_load.rs` still
   pass, and a single-endpoint group is not measurably slower than the M1
   baseline (see section 8).

## 2. Scope

### In scope

- Multi-endpoint upstream groups with weights; endpoints identified by a stable
  id; per-endpoint admin state (`enabled`, `draining`, `disabled`).
- Load balancing algorithms per group: round robin, weighted round robin, least
  connections, power-of-two-choices with EWMA latency, consistent hash.
- Active health checks (HTTP and TCP) with interval, timeout and rise/fall
  thresholds; passive outlier ejection with exponential backoff.
- Idempotent retries with a per-group retry budget; an all-unhealthy policy.
- Runtime state (health, in-flight, latency) that survives configuration swaps.
- Admin API and UI for all of it; metrics, access log fields, push events.
- The M1 carry-over items in workstream C.

### Out of scope (later milestones)

| Item | Milestone |
|---|---|
| TLS to upstreams, HTTP/2 to upstreams, TLS health checks | M3 |
| Termination, SNI, ACME | M3, M4 |
| Maintenance mode (per host or route) | M5 |
| Admin authentication, audit log, config import/export | M5 |
| Rate and connection limits per endpoint, circuit-breaker style caps | M7 |
| Request sampling | M6 |
| Proxy-set affinity cookies (consistent hash can *read* a cookie; the proxy never sets one), slow start, latency-based ejection, DNS/SRV service discovery, per-route algorithm overrides, request mirroring | after 1.0 or as the roadmap says |

Endpoint addresses stay `host:port` as in M1. A hostname that resolves to several
IPs is still **one** endpoint (the connector resolves it per connection); making
each IP its own endpoint is service discovery and is out of scope.

## 3. Design decisions for M2

Items marked **(confirm)** are recommendations that need your sign-off.

| # | Decision | Recommendation |
|---|---|---|
| E1 | Endpoint identity | Endpoints get a stable `EndpointId` carried in `Config` and in revision JSON, and **preserved on rollback** (M1 re-inserts them without ids). Runtime state is keyed by it. **(confirm)** |
| E2 | Where runtime state lives | In an `Arc<EndpointRuntime>` (atomics) stored inside each compiled endpoint and **carried from the previous snapshot** when the endpoint id and address are unchanged; a changed address, or a removed endpoint, starts fresh. No global map on the hot path; no state inside the immutable config data. **(confirm)** |
| E3 | Crates | New pure crate `fast-proxy-balancing` (runtime state, selection algorithms, passive-health state machine; depends on `model` only) between `model` and `routing`. New crate `fast-proxy-health` (active probers, I/O) beside the data plane. The data plane still never touches the store. **(confirm)** |
| E4 | Default algorithm | `round_robin`. Consistent hash needs a key source: `client_ip` (default), a header name, or a cookie name. |
| E5 | Weights | Integer `1..=1000`. Zero is not allowed (use `disabled`). At most **64 endpoints per group** (bounds per-request and per-compile cost; revisit if needed). |
| E6 | Health defaults | Passive ejection **on** by default: 3 consecutive failures, base ejection 10 s, doubling up to 5 min, never eject more than 50% of a group. Active checks **off** by default (they need a known path); when enabled: interval 5 s, timeout 2 s, rise 2, fall 3, expect HTTP 200-399. **(confirm)** |
| E7 | What counts as a passive failure | Connect error, response timeout, and `502/503/504` from the upstream. A plain `500` is **not** a health signal (it is usually an application bug, not a dead endpoint). **(confirm)** |
| E8 | All endpoints unhealthy | Per-group policy, default **fail open** ("panic mode": ignore health, still honour admin state) so that a false-positive health failure cannot become a total outage; `fail_closed` answers `503 no healthy upstream`. Draining and disabled endpoints are never used, even in panic mode. **(confirm)** |
| E9 | Draining versus disabled | Both stop **new** requests and let in-flight ones (and open WebSocket tunnels) finish. They differ only in meaning and display: `draining` is "leaving soon" and the UI shows when it is fully drained (in-flight 0); `disabled` is "parked". Collapse them into one state if that distinction is not worth keeping. **(confirm)** |
| E10 | Retry defaults | Max 2 attempts (one retry). Retry **connect-phase failures for any method** (the request was never sent); retry **idempotent methods** (`GET`, `HEAD`, `OPTIONS`, `PUT`, `DELETE`, `TRACE`) on `502/503/504`. Timeouts are opt-in. Never retry upgrades, and never retry once any request body byte has been sent. Each retry avoids the endpoint that just failed. **(confirm)** |
| E11 | Retry budget | Per group: retries may not exceed 20% of recent requests, with a floor of 10 retries per second, so a failing group cannot be hammered by its own retries. |
| E12 | Request bodies and retries | `Incoming` cannot be cloned. Retrying a connect failure for a request with a body needs the body back intact: a small "lendable" body wrapper that the client polls and that can be reclaimed if it was never polled. **Gated by spike G2**; if the spike fails, only bodyless requests are retried. |
| E13 | Resolver output | The resolver (`POST /api/resolve`) shows the group's algorithm and its **eligible endpoints with their states**, not a single pick, because the pick is per-request (and random for some algorithms). |
| E14 | Metric labels | `endpoint` is the endpoint's `host:port`, a config value, so label cardinality stays bounded by configuration (rule from M1 holds). Series for removed endpoints are deleted. |
| E15 | Time in state machines | Passive and active state machines take `now` as a parameter (no hidden clock), so tests are deterministic and need no sleeping. |
| E16 | API compatibility | None kept: the project is undeployed. The upstream resource's `addr`/`weight` move into `endpoints`; the schema file is edited in place (version bump, no migration). |

## 4. Target architecture

```
                               config.toml (unchanged)           SQLite
                                      |                             ^
                                      v                             |
 +--------------------------- main ---------------------------------+---------+
 |                                                                            |
 |  data plane (hyper)                        admin plane (axum)              |
 |    |  load Snapshot (arc-swap)               REST + WS + UI + /metrics     |
 |    |    host -> subsystem -> route -> Arc<CompiledUpstream>                |
 |    |                                  |  endpoints[] each holding          |
 |    v                                  |  Arc<EndpointRuntime> (atomics) <--+-- carried across swaps (E2)
 |  select endpoint  (balancing)         |                                    |
 |    eligible = admin Enabled && healthy && not ejected                       |
 |    algorithm picks;  none eligible -> failure policy                        |
 |    |                                                                        |
 |  forward -> record outcome ------> passive health (balancing)              |
 |    |  retry on another endpoint (budget)         ^                          |
 |    v                                             |                          |
 |  your backends  <--- active probes (health) -----+  one task per endpoint   |
 +----------------------------------------------------------------------------+
```

Crate dependencies (one way, no cycles):

```
model <- balancing <- routing <- dataplane
model <- store
routing + balancing + store + telemetry <- config-service
routing + balancing + telemetry <- health
config-service + health + dataplane-facing handles <- admin
everything <- the binary
```

- **`fast-proxy-model`** (pure) gains `EndpointId`, `EndpointState`, weights, and
  the policy types (load balancing, health, failure policy, retry) with
  validation.
- **`fast-proxy-balancing`** (pure, new) owns `EndpointRuntime`, the
  `RuntimeRegistry`, the five selection algorithms, eligibility and panic
  logic, and the passive-health state machine. No I/O, no clock of its own.
- **`fast-proxy-routing`** compiles `Config` into `CompiledUpstream` /
  `CompiledEndpoint` (carrying the runtime `Arc`s forward), replacing the single
  `endpoint` on `CompiledRoute`.
- **`fast-proxy-health`** (new) runs the active probers and reports into the
  runtime state; it reconciles its tasks whenever the snapshot changes.
- **`fast-proxy-dataplane`** selects an endpoint per request, records outcomes,
  retries, and enforces drain/disable and the failure policy.
- **`fast-proxy-store`** / **`config-service`** / **`admin`** / **UI** carry the
  new configuration and expose runtime status.

### 4.1 One request, in order

1. Load the snapshot once (as in M1); match host, subsystem, route.
2. The route's `Arc<CompiledUpstream>` lists its endpoints with their runtime.
3. Build the request context (client IP, headers for hash keys).
4. Select: filter to **eligible** endpoints (admin `enabled`, health `healthy`,
   not ejected); the group's algorithm picks one. If none is eligible, apply the
   failure policy (panic: pick among admin-`enabled` ignoring health; or 503).
5. Rewrite the URL and `Host` for the **chosen** endpoint (today this reads the
   route's single endpoint), forward, stream the response.
6. Record the outcome on the endpoint (success/failure, latency to first byte)
   for passive health and EWMA; the in-flight count is held until the response
   finishes.
7. If the attempt failed in a retryable way and the policy and budget allow,
   select again **excluding** the endpoint that failed, and go to 5.
8. The access log line carries the endpoint actually used and the attempt count.

### 4.2 Endpoint health: states and transitions

An endpoint's **effective** availability combines two independent things:

- **Admin state** (configuration, in the database, survives restarts):
  `enabled`, `draining`, `disabled`. Only `enabled` receives new requests.
- **Health** (runtime, in memory): `healthy` or `unhealthy`, set by active
  probes (rise/fall) and by passive ejection.

```
passive:   healthy --(N consecutive failures)--> ejected (for base * 2^k, capped)
                                                    |
                          ejection expires -> half-open: one trial request
                                                    |-- success --> healthy (k resets)
                                                    '-- failure --> ejected again (k+1)

active:    healthy --(fall failed probes)--> unhealthy --(rise good probes)--> healthy

guard:     passive ejection is skipped if it would eject more than
           max_ejection_percent of the group (default 50%)
```

Active and passive both feed the same `healthy` flag; an endpoint is routable
only if active says healthy **and** it is not currently ejected. New endpoints
start healthy (optimistic) and are demoted after `fall` failed probes, so
creating a group does not cause a window of 503s.

### 4.3 Selection algorithms

| Algorithm | How it picks | State used |
|---|---|---|
| `round_robin` | Next eligible endpoint in order | one atomic counter |
| `weighted_round_robin` | Smooth weighted rotation over eligible endpoints | atomic counter, weights |
| `least_connections` | Lowest `in_flight / weight`, random tie-break | per-endpoint in-flight |
| `p2c_ewma` | Two random distinct eligible endpoints; the lower of `ewma_latency * (in_flight + 1) / weight` | in-flight, EWMA latency |
| `consistent_hash` | Hash of the key (client IP, header, or cookie) on a ring of virtual nodes proportional to weight; walks to the next eligible node | precomputed ring per group |

Selection must stay cheap: the target is under 200 ns for a three-endpoint group
and a few microseconds for 64, with no allocation (see task B8). A request with
no usable hash key falls back to round robin.

## 5. Data model changes

The schema file is edited in place (version 1 to 2, no migration, per the
project rule). Sketch, not final:

```sql
ALTER upstream_group   -- shown as additions; edit CREATE TABLE directly
  lb_algorithm        TEXT    NOT NULL DEFAULT 'round_robin',
  lb_hash_key         TEXT    NOT NULL DEFAULT 'client_ip',   -- client_ip | header:<name> | cookie:<name>
  failure_policy      TEXT    NOT NULL DEFAULT 'fail_open',    -- fail_open | fail_closed
  health_check        TEXT    NOT NULL DEFAULT 'none',         -- none | http | tcp
  health_path         TEXT    NOT NULL DEFAULT '/',
  health_status_min   INTEGER NOT NULL DEFAULT 200,
  health_status_max   INTEGER NOT NULL DEFAULT 399,
  health_interval_ms  INTEGER NOT NULL DEFAULT 5000,
  health_timeout_ms   INTEGER NOT NULL DEFAULT 2000,
  health_rise         INTEGER NOT NULL DEFAULT 2,
  health_fall         INTEGER NOT NULL DEFAULT 3,
  passive_failures    INTEGER NOT NULL DEFAULT 3,              -- 0 disables passive ejection
  passive_base_ms     INTEGER NOT NULL DEFAULT 10000,
  passive_max_ms      INTEGER NOT NULL DEFAULT 300000,
  passive_max_percent INTEGER NOT NULL DEFAULT 50,
  retry_budget_percent INTEGER NOT NULL DEFAULT 20,
  retry_budget_min_per_sec INTEGER NOT NULL DEFAULT 10

ALTER upstream_endpoint
  state               TEXT    NOT NULL DEFAULT 'enabled'       -- enabled | draining | disabled
  -- id is preserved on rollback (E1); UNIQUE (group_id, addr)

ALTER route
  retry_max_attempts  INTEGER NOT NULL DEFAULT 2,              -- 1 = never retry
  retry_idempotent_on_5xx INTEGER NOT NULL DEFAULT 1,
  retry_on_timeout    INTEGER NOT NULL DEFAULT 0
```

Rules enforced in code (and by the test suite):

- A group has **at least one** endpoint and at most 64 (replaces M1's "exactly
  one"); addresses are unique within a group; weights are `1..=1000`.
- The last endpoint of a group cannot be deleted (delete the upstream instead).
- Policy values are validated with field-specific messages (`lb.algorithm`,
  `health.intervalMs`, ...); field paths for nested objects use dotted camelCase
  (`endpoints.0.addr`).
- Revision JSON carries the new fields and endpoint ids, and rollback restores
  them exactly.

### API shape (sketch)

The upstream resource in `GET /api/subsystems/{id}` and its writes:

```json
{ "id": 5, "name": "backend",
  "lb": { "algorithm": "p2c_ewma", "hashKey": "client_ip" },
  "failurePolicy": "fail_open",
  "health": { "check": "http", "path": "/healthz", "expectedStatus": [200, 399],
              "intervalMs": 5000, "timeoutMs": 2000, "rise": 2, "fall": 3 },
  "passive": { "consecutiveFailures": 3, "baseEjectionMs": 10000,
               "maxEjectionMs": 300000, "maxEjectionPercent": 50 },
  "retryBudget": { "percent": 20, "minPerSec": 10 },
  "endpoints": [ { "id": 11, "addr": "10.0.0.1:8080", "weight": 1, "state": "enabled" } ] }
```

New endpoints: `POST /api/subsystems/{id}/upstreams/{uid}/endpoints`,
`PUT|DELETE .../endpoints/{eid}`, `PUT .../endpoints/{eid}/state`. Runtime (not
configuration, so a separate read):
`GET /api/subsystems/{id}/upstreams/{uid}/status` returns, per endpoint, the
effective state and why (`healthy`, `unhealthy: 3 failed probes`,
`ejected until ...`, `draining`, `disabled`), in-flight count, EWMA latency, last
probe result and time. The WebSocket gains an `endpointState` event.

## 6. Tasks

Task IDs are referenced by the dependency table in section 7. Workstream letters:
**G** groundwork, **MD** model, **S** store, **B** balancing, **R** routing and
compile, **D** data plane, **H** health, **T** telemetry, **A** admin API, **U**
admin UI, **Q** quality, **W** docs, **C** carry-over from M1.

### Workstream G: Groundwork

- [x] **G1 (S) Decisions and ADRs.** Get the **(confirm)** items in section 3
  resolved. Write `docs/adr/` notes: 0005 runtime state and stable endpoint
  identity (E1-E3), 0006 selection algorithms and eligibility (E4, E5, E8, E9),
  0007 health and ejection (E6, E7, E15), 0008 retries, budget and body replay
  (E10-E12).
  *Done when:* the ADRs are merged and no **(confirm)** item is open.
- [x] **G2 (M) Spike: request body replay and pooled-connection failures.**
  Verify with a throwaway test: (a) the hyper-util legacy client does **not**
  poll the request body before a connection is established, so a connect
  failure leaves a never-polled body reclaimable; (b) what error a request gets
  when a reused pooled connection was closed by the peer, and whether the body
  was already polled; (c) that a "lendable" body wrapper works (reclaim after a
  connect failure, no reclaim after the first poll).
  *Done when:* findings are in ADR 0008 and E12 is confirmed or reduced to
  "bodyless requests only". (Confirmed in PR7.)
- [x] **G3 (S) Decide the defaults table.** Turn E4-E11 into the documented
  defaults (section 5 and the admin docs) once confirmed; this is the single
  source the model validation and UI forms use.

### Workstream MD: Model

- [x] **MD1 (M) Endpoint identity, state and weights.** `EndpointId`,
  `EndpointState`, `EndpointConfig { id, addr, weight, state }`; weight
  `1..=1000`; at most 64 endpoints per group.
  *Done when:* table-driven tests accept and reject each case with a
  field-specific message.
- [x] **MD2 (M) Group policy types.** `LbPolicy` (algorithm, hash key source),
  `HealthCheck` (none/http/tcp, path, status range, interval, timeout, rise,
  fall), `PassiveHealth`, `FailurePolicy`, `RetryBudget`; route `RetryPolicy`.
  All validated, with bounds (for example interval 100 ms to 10 min, rise and
  fall 1 to 10). Pure, no I/O.
  *Done when:* every bound is covered by accepted/rejected tests and the
  defaults match G3.

### Workstream S: Store

- [x] **S1 (M) Schema.** Edit `schema.sql` in place with the columns in
  section 5; bump `SCHEMA_VERSION`; unique `(group_id, addr)`.
  *Done when:* a fresh database opens, and an old-version file is rejected with
  the existing clear error.
- [x] **S2 (M) Repository operations.** Endpoint create / update / delete /
  set-state; group settings update; group create takes its endpoints atomically
  (at least one); refuse deleting a group's last endpoint (`Conflict`). Each
  write still goes through `Database::write` and records a revision.
  *Done when:* success and failure tests cover uniqueness, the last-endpoint
  rule, cascade, and revision rows.
- [x] **S3 (M) Revisions with endpoint ids and policies.** Extend the revision
  JSON; make `restore` keep endpoint ids (E1).
  *Done when:* a test edits endpoints and policies, rolls back to each prior
  revision, and compares `load_all()` (including ids) to the stored config.

### Workstream B: Balancing (new pure crate)

- [x] **B1 (M) Crate and runtime state.** `EndpointRuntime` (atomics: in-flight,
  health flag, passive counters, ejected-until, EWMA latency), `RuntimeRegistry`
  (the carry-over map by endpoint id and address), `Candidate`, request context
  (client IP, header/cookie lookup), and a `Selection` result.
  *Done when:* unit tests cover state updates and registry reuse/reset rules (E2).
- [x] **B2 (S) Round robin and weighted round robin.** (weighted rotation is in blocks per cycle, exact over a full cycle, not interleaved)
  *Done when:* a test shows the pick counts match the weights over many picks
  (exact for a full cycle), and skipping ineligible endpoints keeps the rotation
  fair among the rest.
- [x] **B3 (S) Least connections.**
  *Done when:* with fixed in-flight counts it picks the minimum weighted load and
  breaks ties randomly (seeded in tests).
- [x] **B4 (M) Power of two choices with EWMA.** Peak-EWMA scoring with a decay
  time constant; sensible cold start (no samples yet).
  *Done when:* tests show a consistently slower endpoint receives a smaller
  share, and a fresh endpoint is not starved.
- [x] **B5 (M) Consistent hash.** Ring of virtual nodes proportional to weight,
  key from client IP, header or cookie; walks to the next eligible node; falls
  back to round robin without a key.
  *Done when:* property tests show the same key maps to the same endpoint, and
  removing one of N endpoints remaps only about 1/N of keys.
- [x] **B6 (M) Eligibility and the failure policy.** `eligible = admin enabled
  && healthy && not ejected`; none eligible: fail open picks among admin-enabled
  endpoints ignoring health, fail closed returns none; draining and disabled are
  never picked.
  *Done when:* a property test shows an ineligible endpoint is never chosen while
  an eligible one exists, for every algorithm.
- [x] **B7 (M) Passive health state machine.** Consecutive failures, ejection
  with exponential backoff, half-open single trial, reset on success, and the
  `max_ejection_percent` guard. Takes `now` explicitly (E15).
  *Done when:* deterministic tests walk every transition, including the guard.
- [x] **B8 (S) Selection overhead check.** A timing test or microbenchmark for
  every algorithm at 3 and 64 endpoints.
  *Done when:* results are recorded in `docs/benchmarks.md` and meet the budget
  in 4.3 (or the budget is revisited with the numbers).

### Workstream R: Routing and compile

- [x] **R1 (M) Compiled upstream types.** `CompiledUpstream` (name, policies,
  endpoints, precomputed ring and weight tables) and `CompiledEndpoint` (id,
  address, weight, admin state, `Arc<EndpointRuntime>`); `CompiledRoute` holds an
  `Arc<CompiledUpstream>` instead of a single endpoint (routes of the same group
  share it).
- [x] **R2 (M) Compile with carried runtime.** `compile(version, config,
  &registry)` reuses an endpoint's runtime when its id and address match and
  starts fresh otherwise; replaces the "exactly one endpoint" rule with "at
  least one, at most 64, unique addresses".
  *Done when:* tests show state survives unrelated edits, resets when an
  address changes or the endpoint is removed, and a bad group is rejected.
- [x] **R3 (S) ConfigService owns the registry.** It passes the registry into
  every compile and publishes the new snapshot as before.
  *Done when:* a service test changes an unrelated route and sees an endpoint's
  ejection state unchanged, and sees it reset after the endpoint's address is
  edited.
- [x] **R4 (S) Lookup and resolver data.** The resolver reports the group's
  algorithm and its eligible endpoints with states (E13); `build_upstream_uri`
  takes the chosen endpoint.

### Workstream D: Data plane

- [x] **D1 (L) Select per request.** `UpstreamClient::forward` builds the request
  context, selects, and rewrites the URL and `Host` for the chosen endpoint.
  *Done when:* an end-to-end test with three in-process backends sees traffic
  spread per algorithm, and `preserve_host = false` sends the chosen endpoint's
  address.
- [x] **D2 (M) In-flight and latency accounting.** An RAII in-flight guard held
  until the response finishes (including streamed bodies), and EWMA updated at
  response head.
  *Done when:* tests show the in-flight count returns to zero after normal,
  aborted and upgraded requests.
- [x] **D3 (M) Passive outcomes.** Record connect error, timeout and
  `502/503/504` as failures and everything else as success (E7); ejection and
  recovery become visible in tests with a fake backend.
  *Done when:* killing a backend ejects it after the configured failures and a
  revived one returns after its ejection expires.
- [x] **D4 (M) Failure policy and the new fixed response.** (`503 no_healthy_upstream`, fail-open panic mode counted in `fastproxy_upstream_panic_total`; draining and disabled endpoints never used)
  upstream` (JSON code `no_healthy_upstream`) for fail-closed groups; panic-mode
  picks counted.
  *Done when:* tests cover all-unhealthy with each policy, and with all
  endpoints draining or disabled (always 503, never panic mode).
- [x] **D5 (L) Retries.** Attempt loop with endpoint exclusion, the idempotency
  and status rules (E10), the per-group budget (E11), and body replay as decided
  in G2; never for upgrades or after body bytes were sent.
  *Done when:* tests show a `GET` retried on `503`, a `POST` retried only on
  connect failure, no retry after a body byte, the budget refusing a retry storm,
  and `attempts` correct in the access log.
- [x] **D6 (S) Access log and summary.** `upstream` is the endpoint actually
  used (last attempt) and a new `attempts` field is added; update the log
  format tests and docs.
- [x] **D7 (S) Drain and disable semantics.** New requests skip the endpoint;
  in-flight requests and upgraded tunnels finish.
  *Done when:* a test holds a slow request on an endpoint, drains it, sees no new
  traffic reach it while the slow one completes.

### Workstream H: Health (new crate)

- [x] **H1 (M) Prober engine.** One task per endpoint of a group with active
  checks, started and stopped as snapshots change (a removed endpoint or a
  changed check config cancels its task), start jitter so probes do not
  synchronize, and a bound on concurrent probes.
  *Done when:* tests show probers appear, change and disappear with config
  edits and stop at shutdown.
- [x] **H2 (M) HTTP and TCP probes.** HTTP (path, expected status range, timeout)
  and TCP connect, feeding the rise/fall state machine.
  *Done when:* a backend toggled between healthy and failing is marked
  unhealthy after `fall` probes and healthy after `rise`, with bounded timing.
- [x] **H3 (S) State change events.** (the `EndpointEvents` bus carries health and ejection changes and the admin plane forwards them as `endpointState` WebSocket events and activity entries; admin-state changes arrive as `ConfigChanged`) Broadcast every endpoint state change
  (health, ejection, admin state) for the admin plane, metrics and logs.
- [x] **H4 (S) Startup behaviour.** New endpoints start healthy and are demoted
  after `fall` failures (section 4.2); a restart rebuilds state from scratch.

### Workstream T: Telemetry

- [x] **T1 (M) Upstream metrics.** (all of the listed metrics exist; labels from config only, series of removed endpoints and groups are deleted)
  `..._in_flight`, `fastproxy_upstream_requests_total` (by outcome),
  `fastproxy_upstream_retries_total` (by reason),
  `fastproxy_retry_budget_exhausted_total`, `fastproxy_health_checks_total`,
  `fastproxy_endpoint_ejections_total`, `fastproxy_upstream_panic_total`;
  labels from config only (E14); series for removed endpoints are deleted.
  *Done when:* a scrape after known traffic asserts the counters, and removing an
  endpoint removes its series.
- [x] **T2 (S) Status and traffic.** (`GET /api/status` has the upstream-health summary and the dashboard shows it as the Upstream health card)
  total endpoints, the unhealthy list) to `GET /api/status` and the live traffic
  snapshot for the dashboard.

### Workstream A: Admin API

- [x] **A1 (M) Upstream group resource.** Create with endpoints; update name and
  policies; validation errors name nested fields (`endpoints.0.addr`,
  `health.intervalMs`).
- [x] **A2 (M) Endpoint resource.** Add, update, remove, and `state`
  (enable/drain/disable); removing the last endpoint is a `409`.
- [x] **A3 (S) Route retry settings** in the route API.
- [x] **A4 (M) Runtime status.** `GET .../upstreams/{uid}/status` with the
  effective state and reason per endpoint, in-flight, EWMA, last probe; and the
  `endpointState` WebSocket event plus an activity entry.
- [x] **A5 (S) Resolver** shows the group and its eligible endpoints (E13).

*Done for A1-A5:* every endpoint has HTTP-level tests in the style of
`crates/fast-proxy-admin/src/api_tests.rs` (success, validation `fields`, 404,
409), and a test proves each write is visible to the data plane without a
restart.

### Workstream U: Admin UI

- [x] **U1 (S) API client, types and validation** for the new resources and
  fields (`lib/types.ts`, `lib/api.ts`, `lib/validation.ts`).
- [x] **U2 (L) Upstream section.** Each group shows its algorithm and a health
  summary and an endpoint table (address, weight, state badge, in-flight, last
  check); add, edit and remove endpoint dialogs; enable / drain / disable actions
  whose confirmations say what will happen (and, for drain, when it is drained).
- [x] **U3 (M) Group settings form.** Algorithm (and hash key), failure policy,
  active health check, passive ejection and retry budget, with the defaults from
  G3 and an "advanced" fold so the common case stays simple.
- [x] **U4 (S) Route form: retries** (attempts, retry on timeout).
- [x] **U5 (M) Live status.** Refresh endpoint status from `endpointState`
  events; a dashboard card for upstream health (healthy of total, which are
  unhealthy and why).
- [~] **U6 (S) Revisions viewer and resolver** show endpoints and policies. (the resolver shows the algorithm and each endpoint's eligibility; the revision viewer lists every endpoint but not the policy settings yet)
- [x] **U7 (S) Tests and build.** Vitest tests for the forms (validation shown,
  submit sends the right body), state-change confirmations, and live updates
  from events; `npm test` and `npm run build` pass.

### Workstream Q: Quality

- [x] **Q1 (L) Test backends that can fail on demand.** (the data plane's `TestUpstream` and `tests/common/flaky.rs`: kill, revive on the same port, slow, fail with a status, fail health probes only, hit counts; no Unix-only commands) Extend the in-process
  backends in `tests/common/mod.rs` and the data plane's test upstream: "kill"
  (stop listening and drop connections), revive on the same port, slow, answer
  `503` on demand, count hits. No Unix-only commands, so tests run on Windows.
- [x] **Q2 (L) Exit-criteria end-to-end tests** (`tests/`): three endpoints and
  the distribution per algorithm; kill one under load and assert the bounded
  detection time and no client errors for retryable requests; revive and assert
  automatic return; drain and disable; fail-open and fail-closed; passive and
  active detection each. Each must be shown to be able to fail (break the
  behavior, see red).
- [x] **Q3 (M) Reload-under-load v2.** Extend `tests/reload_under_load.rs`:
  endpoints added, removed, drained and re-enabled while clients run. Assert zero
  failed requests, and the sharp rule from M1 applied to endpoints: after the
  write that disables or drains an endpoint returns, no *new* request reaches it.
- [x] **Q4 (M) Property and unit coverage** listed under B (distribution,
  hash remapping, eligibility, state machines); keep overall coverage above 60%
  and report it (aim to stay near M1's 97%).
- [x] **Q5 (S) Benchmark.** Re-run `docs/benchmarks.md` with a one-endpoint group
  (compare with the M1 baseline) and with 3 and 16 endpoints per algorithm;
  record the numbers and the cost of selection and retries enabled.
- [x] **Q6 (S) Gates.** fmt, clippy, tests and UI build green on Linux, macOS and
  Windows in CI (the M1 lessons apply: one write per raw request with
  `TCP_NODELAY`, signal tests Unix-only, free-port retry).

### Workstream W: Documentation

- [x] **W1 (M) README and getting-started.** A multi-endpoint walkthrough
  (three Python backends, kill one, watch it leave and return; drain one); the
  config and API tables; the metrics list.
- [x] **W2 (S) AGENTS.md.** New crates and rules: the data plane still never
  touches the store; runtime state lives in `fast-proxy-balancing` and is carried
  across swaps by endpoint id and address; state machines take `now`.
- [x] **W3 (S) ROADMAP.md.** Tick M2, record decisions E1-E16 in the decision
  log, update the risks table, link this plan.
- [x] **W4 (S) Outcome.** A closing section in this file like M1's section 10.

### Workstream C: Carry-over from M1

These are small and independent; schedule them where they fit.

- [x] **C1 (S) Requests cancelled before the response head** (the client left
  first) are counted and logged (for example status `499`, `client_closed`),
  using the same pipeline this milestone is changing.
- [x] **C2 (S) Reload cost.** Measure config reload time with several thousand
  routes and endpoints (compile now also builds rings and carries runtime state);
  record it. Target: milliseconds, with no effect on in-flight latency.
- [ ] **C3 (S) Owner click-through of the M1 UI** in a real browser (desktop and
  phone width, light and dark) before M2's UI work builds on it.

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| G1 | none (needs the owner's answers) |
| G2 | none (a spike; can start now) |
| G3 | G1 |
| MD1, MD2 | G3 (defaults), G1 |
| S1-S2 | MD1, MD2 |
| S3 | S2 |
| B1 | MD1 |
| B2-B6 | B1 |
| B7 | B1 |
| B8 | B2-B5 |
| R1 | B1, MD1, MD2 |
| R2 | R1, S3 |
| R3 | R2 |
| R4 | R2 |
| D1 | R2, B2-B6 |
| D2 | D1 |
| D3 | D1, B7 |
| D4 | D1, B6 |
| D5 | D3, G2 |
| D6 | D1, D5 |
| D7 | D1 |
| H1 | R3, B7 |
| H2 | H1 |
| H3 | H1, D3 |
| H4 | H1 |
| T1 | D3, H3 |
| T2 | H3 |
| A1-A3 | R3, S2 |
| A4 | H3, D2, T2 |
| A5 | R4 |
| U1 | A1 (shapes can be agreed up front) |
| U2-U4 | A1-A3 |
| U5 | A4 |
| U6 | A5 |
| U7 | U2-U6 |
| Q1 | none (test support) |
| Q2 | D5, D7, H2, Q1 |
| Q3 | D7, A2, Q1 |
| Q4 | B2-B7 (and the code each test covers) |
| Q5 | D5, B8 |
| Q6 | every code task (runs in CI on each PR) |
| W1-W4 | feature tasks complete |
| C1 | D1 (same code path) |
| C2 | R3 |
| C3 | none (the owner's check; do it before U2) |

Suggested slicing into pull requests, each leaving `main` green and releasable
(the M1 stack was merged only at the end; for M2 prefer merging each PR to
`main` as it passes CI, so the stack stays short):

1. **PR1** G1, G3 (decisions and ADRs); G2 spike as its own throwaway branch
2. **PR2** MD1-MD2, S1-S3 (model, schema, repository, revisions with ids)
3. **PR3** B1-B8 (the pure balancing crate), in parallel with PR2
4. **PR4** R1-R4 (compile with carried runtime; resolver data), plus the core of D1 and D4's fixed response: the data plane must select an endpoint for the stack to stay green
5. **PR5** D2, D6, D7 and the rest of D4 (accounting, access log, drain) with Q1
6. **PR6** D3 (passive health), T1's passive metrics and the panic-pick counter from D4
7. **PR7** D5 (retries and budget)
8. **PR8** H1-H4, rest of T1, T2 (active health crate); events reach the admin WebSocket in PR9
9. **PR9** A1-A5 (admin API)
10. **PR10** U1-U7 (UI)
11. **PR11** Q2-Q6, W1-W4, C1-C3, release `v0.3.0`

PRs 2 and 3 can proceed in parallel after PR1; PR5 needs both plus PR4.

## 8. Performance and cost budget

- A single-endpoint group must stay within about 5% of the M1 baseline
  (`docs/benchmarks.md`: roughly 25 us added at p50 on one connection); a
  slower result needs an explanation or a fix before release.
- Selection: under 200 ns at three endpoints and a few microseconds at 64, with
  no allocation (B8). Consistent hash rings are built at compile time, never per
  request.
- Compile cost grows with endpoints (rings, runtime carry-over); reload with
  thousands of routes and hundreds of endpoints must still be milliseconds (C2).
- Active probes must not add measurable load: jittered, bounded concurrency,
  small requests.

## 9. Risks specific to M2

| Risk | Mitigation |
|---|---|
| The hyper client may poll a request body before connecting, or a pooled connection may fail in a way that is not safe to retry, which limits retries | Spike G2 first; fall back to retrying only bodyless requests and connect failures that provably sent nothing |
| Retries amplify an outage (retry storms) | Per-group budget (E11), retries avoid the failed endpoint, timeouts opt-in, metrics on exhausted budget |
| Health flapping or false positives take healthy endpoints out | rise/fall hysteresis, exponential ejection backoff, `max_ejection_percent`, fail-open default (E8), state-change events and metrics to see it |
| Runtime state lost or mixed up across config swaps or rollbacks | Stable endpoint ids kept on rollback (E1) and carry-over keyed by id and address (E2), with tests for reuse and reset |
| Selection cost or contention on the hot path | Atomics only, no locks; consistent-hash rings precomputed; B8 timing budget; Q5 benchmark against the M1 baseline |
| Thousands of probe tasks or synchronized probes | Jitter, bounded concurrency, one task per endpoint only when active checks are on, 64-endpoint cap per group |
| Stale pooled connections to a dead endpoint cause extra failures until the pool notices | Retry connect-phase and closed-connection failures on another endpoint; passive ejection stops new attempts quickly; revisit pool idle settings if tests show a long tail |
| Time-dependent tests are slow or flaky | State machines take `now` (E15); end-to-end tests use short configured intervals and bounded waits, not long sleeps |
| Metric series leak for removed endpoints; cardinality grows | Delete series on removal (T1), labels only from config (E14) |
| UI complexity: many settings on one form | Defaults from G3, an "advanced" fold, forms that show server field errors inline as in M1 |
| Scope creep into M3 (TLS to upstream), M5 (maintenance) or service discovery | New ideas go to the roadmap, not this plan; section 2 lists what is out |
| Windows CI differences (signals, timing) | Tests kill backends in-process rather than with shell commands; no Unix-only tests unless gated |
| The UI from M1 has not been checked in a real browser | C3 before building M2 UI on top of it |

## 10. Definition of done for M2

- [x] Every task above is checked, or explicitly moved with a note (C3 is the owner's; U6 is partly done, see section 11).
- [x] Exit criteria (section 1) demonstrated by the end-to-end tests in Q2 and Q3
  (`tests/upstream_groups.rs`, `tests/reload_under_load.rs`). The manual run in the
  UI (three backends, kill one, watch it leave and return) is still the owner's, with C3.
- [x] CI green on all three platforms; no new clippy warnings.
- [x] Overall test coverage above 60%, number recorded (AGENTS.md rule): 97.7% of lines (96.8% of regions).
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [x] README, AGENTS.md, ROADMAP.md and getting-started updated.
- [x] Benchmark re-run and recorded; single-endpoint overhead within budget (docs/benchmarks.md).
- [ ] Release `v0.3.0` prepared through the existing release workflow.

## 11. Outcome

M2 is built. An operator creates an upstream group with several endpoints in
the admin UI (or API), traffic is spread by the group's algorithm, a killed
backend leaves rotation without client errors and returns by itself, endpoints
can be drained or disabled, and all of it is visible in metrics, the access log,
the UI and the WebSocket. It was delivered as a stack of eleven pull requests
(decisions and ADRs; model and schema; the pure balancing crate; the compiled
upstream groups; in-flight accounting; passive health; retries; the health
crate; the admin API; the UI; the end-to-end tests, docs and wrap-up).

### Exit criteria, and where each is shown

| # | Criterion | Evidence |
|---|---|---|
| 1 | Three endpoints, traffic spread by the algorithm | `tests/upstream_groups.rs`: exact for round robin (10/10/10) and weighted (30/10/10), sticky for consistent hash, all served for least connections and P2C |
| 2 | Killing one of three under load causes no sustained errors; bounded detection | `killing_one_of_three_backends_under_load...`: four clients, zero failures, the dead endpoint ejected after three failures, no further attempts on it |
| 3 | Automatic return after recovery | passive (`a_revived_backend_returns...`) and active (`active_checks_mark_an_endpoint_unhealthy_and_healthy_again_with_no_client_traffic`, found with no client traffic at all) |
| 4 | Drain and disable survive and behave | in-flight request finishes, no new ones arrive, state survives a restart; `reload_under_load.rs` adds the sharp rule: no request that starts after the write returns reaches the endpoint it took out |
| 5 | All-unhealthy policy | fail open serves (counted as panic mode), fail closed answers 503, draining everything is 503 even when failing open |
| 6 | Observable | metrics, `attempts` and the answering endpoint in the access log, `endpointState` events, the status page and dashboard card |
| 7 | No regression | M1 exit criteria and reload-under-load still pass; a one-endpoint group matches the M1 baseline within noise (docs/benchmarks.md) |

### Numbers

544 Rust tests and 239 UI tests pass; line coverage is 97.7%. A single-endpoint
group costs about 27 us at p50 (25 us in M1) and holds the M1 throughput; 3 or
16 endpoints cost 0 to 5%. Endpoint selection takes 4 to 106 ns. Compiling a
4,000-route, 600-endpoint configuration takes 0.6 ms and a whole change 7.5 ms.

### Where the result differs from the plan

- **PR boundaries moved.** Compiling groups forced the data plane to choose an
  endpoint in PR4, so D1 and the 503 response landed there; D2, D6 and D7 followed
  in PR5. Retries (D5) needed spike G2 first.
- **Retries go slightly beyond E10 (ADR 0008):** idempotent requests are also
  retried after a connection reset, not only on 502/503/504. A POST is retried
  only after a failed connect.
- **A fix the end-to-end tests forced:** the power-of-two-choices cold start
  assumed an unsampled endpoint takes 1 ms, so faster measured endpoints starved
  it (an endpoint got 0 of 60 requests). An unsampled endpoint now takes its
  rival's latency in the comparison, so only load decides.
- **`Forward` returns more than a response:** the answering endpoint, the attempt
  count and the in-flight guard, so the access log and in-flight counts follow
  the request to the endpoint that actually served it.
- **The retry budget lives on the compiled group**, so a config change starts it
  empty (at most ten seconds of history lost).
- **Windows needed two test fixes** (a refused connection takes about a second to
  be reported there); both were fixed by waiting for events, not durations.
- **Requests cancelled before the response head** (an M1 limitation, C1) are now
  counted (`fastproxy_requests_cancelled_total`) and logged as 499.

### Known limitations carried forward

- **C3 is open:** the UI has not been looked at in a real browser (desktop and
  phone width, light and dark), including the new endpoint tables, dialogs and the
  folded policy section. Only jsdom tests cover it.
- **U6 is partly done:** the revision viewer lists every endpoint but not the
  group's policy settings.
- **The retry budget and endpoint runtime are per process and in memory;** a
  restart starts health from scratch (new endpoints start healthy).
- **A server-side TLS or HTTP/2 upstream, maintenance mode and admin
  authentication** are M3 to M5 as before; the admin plane still has no
  authentication and binds to loopback.
- **One full-suite run failed once and did not reproduce** in nine later runs
  (six sequential, three in parallel); which test it was is not known.

