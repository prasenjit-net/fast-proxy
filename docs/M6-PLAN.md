# M6 Plan: Request sampling of unusual requests

Status: **draft v0.2 (2026-11-11), proposed; PR1 (ADRs 0020 to 0023, defaults) is the decision record; all (confirm) items accepted by the owner on 2026-11-11, except that J15 and J17 are re-checked by spike G2.** No code yet. v0.2 follows the owner's direction that **all
sample data is stored in the database**, so samples work with several instances sharing one database (J3, J10,
J11, J17, J18; the ring buffer and the spill file of v0.1 are gone). Items marked **(confirm)** need the
owner's sign-off before the tasks that depend on them start.

Parent document: [ROADMAP.md](ROADMAP.md) (section 4.7 and the M6 list). Predecessor: [M5-PLAN.md](M5-PLAN.md)
(complete). M4 (ACME) is still skipped; M6 does not depend on it and leaves it nothing to rework. Task sizes
are relative effort (S under half a day, M about 1-2 days, L about 3-5 days), not commitments. Tick the
checkboxes as tasks land.

## 1. Goal and exit criteria

Today an operator who sees a burst of 502s or a slow route has a metric (that something happened) and an access
log line (the request line, status and timings). Neither says **what the request looked like**: which headers,
what size, what the body was, which attempts were made. Turning on a full capture for everything is too costly
and too dangerous (secrets, memory, latency). M6 adds a **tail-based sampler**: after the response is finished,
the proxy decides whether the request was unusual (or won the small baseline lottery), and if so keeps a
**redacted, size-capped record** of it in the **shared database** (a bounded table that every instance writes to
and every instance can read, so one list shows the samples of the whole cluster and they survive a restart), which
can be browsed, filtered and exported from the admin API and UI.

The ROADMAP's three safety promises are the design constraints: **redaction on by default, bodies opt-in per
route, and never on the request path** (no blocking, bounded memory, bounded rate).

**Exit criteria** (from the roadmap, made measurable):

1. **Tail-based triggers work.** Each trigger below makes a request appear in the sample table with its reason, and a
   request that matches none (and loses the baseline draw) does not: an upstream connectivity failure
   (connect refused or timed out, TLS handshake failure, a reset or timeout before a response, answered by the
   proxy's own 502/504), latency above a static threshold, latency above a
   multiple of the route's rolling p99, a client producing many 4xx, an oversized request, an oversized
   response, unusual headers, a request that only succeeded after a connectivity failure was retried, and the random baseline.
   **An upstream that answers 5xx is not a trigger** (it is the application's own answer, already in the metrics and
   the access log), and a request answered with an upstream's 5xx is sampled only if another trigger matches.
   During an outage the **storm guard** (J19) keeps the table from filling with copies of one failure: a test with a
   dead upstream under load keeps a handful of samples per failing endpoint per minute and counts the rest.
2. **Redaction is on by default and cannot be switched off.** `Authorization`, `Proxy-Authorization`, `Cookie`,
   `Set-Cookie`, the maintenance bypass header and a list of common secret headers are always replaced in every
   stored sample; secret-looking query parameters and body fields are replaced too; operators can only **add**
   names to the lists. Tests send secrets through the real binary and scan the database file and its WAL, the API,
   the export, the logs and the metrics for them (nothing raw is ever at rest, including in a copy of the database).
3. **Bodies are off by default and opt-in per route.** Without the opt-in a sample has no body bytes at all. With
   it, only text content types are kept, truncated to a cap, redacted by field name, and counted against a global
   memory budget so a burst of large bodies cannot exhaust memory.
4. **The sampler never slows or blocks a request.** Decision work is a handful of integer comparisons plus one
   non-blocking channel send for a kept sample; redaction and the **batched database write** happen on a separate
   task, on its own connection. A full queue, or a database that is busy or unavailable, drops the sample and
   counts it; traffic and configuration writes are never affected. Throughput with sampling on at its defaults stays within
   about 2% of the M5 baseline, and under a load where **every** request triggers (a dead upstream, every request a connectivity failure) throughput stays
   within about 5% (section 8).
5. **Cost is bounded.** A per-instance rate limit (token bucket), a bounded hand-off queue, a process-wide body
   memory budget, and a **sample table bounded by retention** (age, row count and total bytes, pruned in small
   batches by every instance). Dropped samples are counted by reason.
6. **Settings are live.** Triggers, thresholds, the baseline rate, the rate limit, redaction additions and the
   per-subsystem and per-route overrides (including body capture) change through the admin API and UI, take
   effect on the next request without dropping a connection, are audited with their actor, and reach other
   instances through the sync poll.
7. **Samples are browsable and exportable.** `GET /api/samples` filters by trigger, status class, subsystem,
   route, instance, client, minimum latency and time; `GET /api/samples/{id}` shows one; `GET /api/samples/export` streams
   NDJSON; the UI has a Samples page with filters, a detail view and an export button. Access is for `admin`
   tokens only (J12), and an export is audited.
8. **It is observable:** captured, dropped (by reason), written, write failures, queue depth and the table's size are metrics; the dashboard
   shows the capture rate.
9. **No regression:** the M1 to M5 exit criteria still pass, and the data plane's hot path loads the snapshot
   once, never touches the database (the sample service does, on its own task and connection), and keeps every
   metric label from configuration names.
10. **Cluster-wide and durable.** With several instances on one database, a sample from any instance appears in
    the list of every instance (each names the instance that served it); samples survive a restart; writing a
    sample never creates a revision and never makes another instance reload its configuration; retention keeps
    the table bounded no matter how many instances write; a locked or unavailable database drops samples (counted)
    and delays nothing else, and a flood of sample writes does not slow configuration writes noticeably (J17).

## 2. Scope

### In scope

- A new pure crate, `fast-proxy-sampling`: the decision (triggers, baseline), the rolling latency window, the
  per-client 4xx counter, the rate limiter, the redactor, the sample encoding and the batcher.
- A new crate, `fast-proxy-sample-service`: the batched database writer, the retention pruner and the queries the
  admin plane reads, on their own connection (J17). The data plane depends on neither it nor the store.
- Model, schema and config-service support for live sampling settings and per-target overrides; the `sample` table.
- Compilation of the effective policy per route into the routing snapshot, with the runtime state (latency
  windows) carried across snapshot swaps.
- Data plane capture: the request head facts, optional body tees, the decision when the response finishes (and
  when a client cancels), and the hand-off to the sampler task.
- Admin API, a WebSocket count event, UI pages and settings, metrics.
- Tests (including the secret hygiene and load tests), the benchmark, a security review, ADRs and docs.

### Out of scope (later milestones, or never)

| Item | Where |
|---|---|
| Sampling admin plane requests | never: the audit log (M5) is the record of admin activity |
| Capturing the contents of upgraded (WebSocket) connections after the handshake | not planned; the handshake request is sampled like any other, the tunnel is not |
| Capturing binary or compressed bodies, decompressing bodies | not planned; only text content types, as sent |
| A separate network store for samples (PostgreSQL, ClickHouse, object storage) | after 1.0; the table is plain portable SQL so the later PostgreSQL project can take it, and sampling at very high rates is bounded by the rate limit anyway |
| A cluster-wide (rather than per-instance) rate limit | not planned; each instance limits itself, the table's retention bounds the total (J18) |
| Replaying a sample against an upstream, HAR export, shadow traffic | post-1.0 candidates (mirroring is already listed there) |
| Shipping samples to an external system (OTLP, Kafka, S3) | after 1.0; the NDJSON export covers it |
| A rotating spill file beside the database | dropped: the database is the durable store (J10); can be revisited if the database proves too busy |
| Alerting on sample rates | not planned; the metrics are the alerting surface |
| Rate limiting of data-plane clients, connection limits | M7 |
| Sampling rules written as expressions (match on header X and path Y) | not planned; fixed triggers and per-route overrides cover the need |

## 3. Design decisions for M6

Items marked **(confirm)** are recommendations that need your sign-off.

| # | Decision | Recommendation |
|---|---|---|
| J1 | Where the decision runs | **When the response has finished**, in the same place the request is already reported (`TrackedBody::drop`, and `PendingRequest::complete` for a client that left first), because that is where the status, the full latency, the bytes and the attempt count are all known. The request side contributes facts only: a compact `RequestHead` (method, path and query, the selected headers, a few flags) taken before forwarding. The pipeline never decides anything before the response. **(confirm)** |
| J2 | Crate layout | New crate **`fast-proxy-sampling`**: pure logic, no I/O, defines a small `SampleSink` trait, depends on `model` only (it takes `now_ms` as a parameter; the data plane passes `monotonic_ms()`). New crate **`fast-proxy-sample-service`** (depends on `sampling`, `store`, `model`): the batched writer, the pruner and the queries (J17). `model` gains the validated settings types; `store` the tables as opaque rows and a sample repository; `routing` compiles the effective policy per route and carries runtime state (it already depends on `balancing`; it now also depends on `sampling`); `config-service` owns the **settings** writes; `dataplane` captures and hands off through the sampler handle; `admin` serves the API (reads through the sample service); `telemetry` the metrics. Dependencies still point one way, and the data plane still never touches the store (it knows only the handle from `sampling`). **(confirm)** |
| J3 | Where settings live | Process settings in `config.toml` (restart): `[sampling] enabled`, `queue_depth`, `batch_size`, `batch_interval`, `body_budget_bytes` (see 5.1). Behaviour in the **database**, live: one global settings row (which also holds the **retention** of the shared table, so every instance prunes by the same rule) and optional **overrides per subsystem and per route**. Like tokens, audit and maintenance (M5 H8) they are **not `Config`**: a rollback never changes them, because it must not silently switch body capture on, or switch redaction additions off. A change is a summary-only revision (so the sync poll picks it up) with an audit entry. **(confirm)** |
| J4 | Triggers and their defaults | A sample records **every** trigger that matched (a list; the first in this order is the primary). Defaults: `upstream_failure` (a **connectivity** failure ended the request: the proxy answered 502 or 504 itself because it could not connect, a connect or response timeout elapsed, the upstream TLS handshake failed, or the connection was reset or closed before a response head) **on**; `retried` (a connectivity failure on one attempt was retried and the request then got an answer) **on**; `slow` (static, 2000 ms) **on**; `slow_p99` (latency above `factor` times the route's rolling p99, default factor 3, floor 50 ms, only after 200 requests in the window) **off**; `client_errors` (one client address crossing 100 4xx responses in a minute) **on**; `oversized_request` (8 MiB) and `oversized_response` (32 MiB) **on**; `unusual_headers` **on**; `baseline` (random, 1 in 1000) **on**. **There is no trigger for a 5xx that an upstream answered**: that is the application's answer, and the metrics and the access log already count it. A retry caused only by an upstream 5xx (ADR 0008 retries idempotent 5xx) is not a `retried` sample, and the proxy's own 502 or 504 is told apart from an upstream's by what the upstream client reports (`upstream_errors.rs` already classifies the error kinds), never by the status. The sample records each attempt's endpoint and failure kind. Trigger names are a fixed enum, so they are safe metric labels. **(confirm)** |
| J5 | Rolling p99 | A per-route `LatencyWindow`: a log-linear histogram (64 buckets of atomics, about 1 ms to 10 minutes) over two one-minute epochs; recording is one `fetch_add`. A sampler tick every 5 seconds computes the p99 of the last one to two minutes and publishes it as one atomic per route, so the request path only reads an integer. It is keyed by `(subsystem name, route prefix)` and **carried across snapshot swaps** like endpoint runtime state (ADR 0005), so a config change does not reset it. Not a metric label, not stored. **(confirm)** |
| J6 | Per-client 4xx counting | A fixed-size table (4096 slots, indexed by a hash of the client IP, a one-minute window per slot, atomic counters). Collisions can only make a trigger fire early for an unrelated client; they never allocate and never grow. The client address is never a metric label and appears only in the sample. On crossing the threshold the crossing request is sampled, then one request in `N` while the client stays above it, so one scanner cannot fill the table. |
| J7 | Unusual headers | Decided at request time from facts that need no allocation: more than 64 headers, a header value over 8 KiB, a header value that is not visible ASCII or valid UTF-8, a duplicated singleton header (`Host`, `Content-Length`, `Content-Type`), `Content-Length` together with `Transfer-Encoding`, an unknown HTTP method, a path over 8 KiB. The result is a bit set in `RequestHead`; it feeds the trigger at the end. The limits are constants with a documented table, not settings (M7 owns request limits). |
| J8 | Redaction | Applied **once, in the sampler task, before a sample is stored or written anywhere**, so nothing raw is ever at rest (not in the database, a copy of it, an export or a log). (a) **Headers:** always-redacted names (`authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token`, `x-amz-security-token`, and the bypass header name of every maintenance scope) plus the operator's additions; the value becomes `[redacted]`, the name and the value length class stay. (b) **Query string:** parameters whose name matches a built-in list (`token`, `access_token`, `refresh_token`, `id_token`, `key`, `api_key`, `apikey`, `secret`, `password`, `passwd`, `signature`, `sig`, `code`, `session`, `ticket`) or an addition become `[redacted]`. (c) **Bodies:** a lexical redactor (it must work on a truncated prefix, so a parse-then-serialize approach is wrong) blanks the values of JSON members and form fields whose names match the built-in list (`password`, `passwd`, `secret`, `token`, `access_token`, `refresh_token`, `client_secret`, `api_key`, `authorization`, `card`, `cvv`, `ssn`, plus additions). If a body cannot be redacted with confidence (an unterminated string at the cut, an unknown encoding), the unsafe tail is dropped, not kept. Overrides and per-route settings can only **add** redaction names, never remove built-in ones. **(confirm)** |
| J9 | Body capture | **Off by default, per route opt-in** (`capture_bodies` on a route override; a subsystem override turns it on for its routes). Only when on: the request body is teed (first `request_cap` bytes, default 4 KiB, maximum 64 KiB) and the response body likewise; only for content types `text/*`, `application/json`, `application/xml`, `application/*+json`, `application/*+xml` and `application/x-www-form-urlencoded`, and only with `Content-Encoding` absent or `identity`; never for upgrades. Because the decision comes after the response, a teed body is held until then; every reserved byte is counted against a **process-wide budget** (`body_budget_bytes`, default 16 MiB): when the budget is spent, the request is simply not captured and the sample says `bodyOmitted: budget`. Bodies are dropped (not stored) for samples that did not trigger. The tee sits on the request body owner (so it never captures a replayed attempt twice: a body is replayable only if it was never polled, ADR 0008) and on the tracked response body. **(confirm)** |
| J10 | Bounded cost and where samples live | **All sample data is stored in the database** (the `sample` table, 5.2), never only in memory. The path is: the **storm guard** (J19), then a **token bucket** (default 20 samples per second per instance, burst 40; checked first, atomics only; drops with `rate_limit`) then a **bounded channel** (`try_send`; full means drop and count `queue_full`), then the sample service redacts, **batches** (default up to 50 samples or 250 ms) and writes one transaction per batch. The per-sample size is bounded (headers capped at 64 entries of 1 KiB, bodies at their caps, path and query at 8 KiB). The hot path allocates nothing for a request that is not sampled. A failed write (busy, full disk, unavailable) drops that batch and counts `store_error`, with a backoff, and never retries without bound. There is no ring buffer and no spill file; a sample is lost only if the process dies before its batch is written (at most one batch interval of samples), which the docs state. **(confirm)** |
| J11 | Cluster and horizontal scaling | Samples are **cluster-wide**: every instance writes the samples of the requests it served into the one shared table, each row carries its `instance` id, and any instance's admin API reads them all, so an operator sees one list whichever instance they reach, and a sample outlives the instance that took it. Row ids are assigned by the database (strictly increasing, shared by all writers), so a cursor is stable across instances. What stays **local** to an instance, because it describes that instance's own traffic: the rolling p99 windows, the per-client 4xx table, the token bucket and the body memory budget (so the limits are per instance and the cluster total is bounded by retention, J18). A sample write is **telemetry, not configuration**: it creates no revision, so it never bumps the cluster generation and never makes another instance reload (J17). Settings and overrides are shared through the database and the revision-version sync poll like everything else. **(confirm)** |
| J12 | Who may see samples | A new action, **`Action::Samples`**, for list, detail, export and clear: **`admin` only**. Samples are redacted but still hold paths, client addresses and, when opted in, body text, which is more than the `viewer` role (M5 H1: every read, no secrets) was promised. A `viewer` can read the **status** (counters, the table's size, the settings; the redaction additions are names, not secrets, so they are readable) through the normal `Read` action. The permission table test enumerates the new pair. Export and clear are audited (`samples.export`, `samples.clear`); settings changes go through `ConfigService::apply` with their audit row (`sampling.update`, `sampling.override.set`, `sampling.override.clear`). **(confirm)** |
| J13 | What is never "unusual" | A maintenance answer (M5) is intended, so it never triggers `upstream_failure` (maintenance answers before any upstream is contacted, M5 H13); a request answered by the fixed unmatched-host 404 or the redirect never triggers on its own (they can still be picked by the baseline and `client_errors`). A bypassed request is sampled like any other. A client that went away is counted as 499 and can be sampled only when another trigger matched (`slow`, `upstream_failure`, `retried`) or by the baseline. |
| J14 | Sample identity | Each sample has `id` (a per-instance, strictly increasing integer), `instance`, `requestId` (the same id as the access log and the response header, so the three can be joined), `at` (wall clock), the primary and all triggers, subsystem and route names (`_unmatched` and `_none` as elsewhere), the client address, the effective `X-Forwarded-For` only as received, method, path, query (redacted), selected headers both ways (redacted), status, timings (total, time to upstream head), attempts and the endpoint address tried, bytes in and out, and the optional bodies. Never a secret, a key or PEM. |
| J15 | Selected request headers | The request side keeps a small allow-list (default: `host`, `user-agent`, `content-type`, `content-length`, `accept`, `accept-encoding`, `referer`, `x-forwarded-for`, `x-request-id`, `origin`) plus the operator's additions, not the whole header map, so retaining it for every request is a few cheap clones, not a full `HeaderMap` copy. Response headers are captured the same way when the sample is built. Spike G2 measures this before the choice is final. **(confirm)** |
| J16 | Compatibility | None kept: the project is undeployed. The schema is edited in place (version 4 to 5), `SCHEMA_VERSION` bumped. |
| J17 | The write path (an exception that needs ADR 0022) | The rule "all writes go through `ConfigService::apply`" is about **configuration**. Samples are high-volume telemetry: one revision, one audit row and one snapshot swap per sample would make every instance reload on every sample. So samples are written by the **sample service** straight through the store, with **no revision, no audit row per sample and no effect on the cluster generation**; changes to the *settings* still go through `ConfigService::apply` as above; `clear` and `export` are single audited events. The sample service uses **its own connection and writer thread** (WAL lets readers run beside it; writers serialize on SQLite's lock, so batches are small and short, one transaction of about 50 rows, and use a busy timeout) so a burst of sample writes cannot queue in front of a configuration write on the store thread. Spike G2 measures configuration-write latency under a heavy sample-write load, across threads **and across processes** on one database file, before this is final. The store crate stays free of cryptography and of any knowledge of what a sample means (it sees opaque columns). **(confirm)** |
| J18 | Retention and growth | Pruning is by the settings row, live and shared: `retention_days` (default 7), `max_rows` (default 100 000) and `max_bytes` (default 256 MiB, measured as the sum of the stored payload lengths), whichever is hit first. **Every instance** runs the pruner every 30 seconds, in batches (delete the oldest 500 rows, repeat while over a limit, yield between batches), which is idempotent, needs no leader election and cannot hold the write lock for long. The running totals come from one cheap indexed aggregate per pass, not a `COUNT(*)` per sample. Between passes the table can exceed a limit slightly; the per-instance rate limit bounds how far. Clearing deletes in batches too. |
| J19 | The storm guard (too many samples during an outage) | A connectivity failure is the one trigger that can fire for **every** request at once (a dead backend, a network partition), so the global rate limit alone is not enough: one outage would use the whole budget, starve every other trigger and fill the table with identical records. Before the global token bucket, each candidate passes a **per-key limiter**. The key is `(subsystem, route, primary trigger)` and, for `upstream_failure` and `retried`, also the **endpoint address and failure kind**, so a dead endpoint is one key, not a thousand. A key gets a **burst of 3 samples, then 1 per 30 seconds** (settings `storm_burst`, `storm_interval_secs`, live). Suppressed candidates are **counted per key**, and the count rides on the next kept sample of that key (`suppressed`, `suppressedSinceMs`), so the table still says how big the outage was; they count as `dropped{reason="storm_guard"}` and cost one atomic. A suppressed candidate consumes **nothing** from the global bucket, so other triggers still get through. The key table is fixed size (1024 slots, hashed, windowed atomics, no allocation; a collision only merges two keys' budgets) and the key is never a metric label. `baseline` is exempt (already random and rare). After a quiet interval a key's next failure is kept again. The guard is per instance, like the global bucket; retention bounds the cluster total (J18). **(confirm)** |

## 4. Target architecture

```
   config.toml [sampling]                         SQLite (shared by instances)
   enabled, queue and batch sizes, body budget    sampling_settings (triggers, thresholds, rate, redaction additions, retention)
                                                  sampling_override (per subsystem or route: enabled, bodies, thresholds)
                                                  revision (summary-only on a settings change)
          |                                       sample (one row per kept request, from every instance)
          v                                                  ^ batched INSERT          ^ SELECT (any instance)
  fast-proxy-sampling (pure)                                 |                         |
  Policy, Trigger, decide(), LatencyWindow,        fast-proxy-sample-service ----------+
  ClientErrors, RateLimiter, Redactor,             writer task + own connection, retention pruner, queries
  SampleSink trait, batcher                                  ^
          ^                                                  | redacted Sample (batches)
          |  RawSample                                       |
   data plane: RequestHead (before forward), decision (TrackedBody drop / PendingRequest),
   token bucket, bounded channel (try_send) ------------> sampler task: redact, size-cap, batch -> SampleSink
          |
   settings: ConfigService::apply -> audit row -> commit -> compile -> swap (Snapshot { route -> CompiledSampling })
          |
          v
   admin API: /api/samples (reads the table), /api/sampling      UI: Samples page, Sampling settings
```

### 4.1 One request, in order

```
pipeline.handle:   host -> subsystem -> maintenance check -> route -> (policy of the route read from the snapshot)
                   RequestHead = flags + selected headers + method/uri        (no allocation when sampling is off for the route)
                   optional body tee on the request body owner               (only when the route opted in, and the budget allows)
forward:           attempts, endpoint, upstream head time                     (already reported by Forwarded)
response body ends (TrackedBody::drop) or the client leaves (PendingRequest::complete):
                   LatencyWindow.record(elapsed)                              (one atomic, always, for routes with sampling on)
                   decide(policy, RequestHead, outcome) -> Option<Triggers>   (integer compares, atomics)
                   if triggers: token bucket -> body budget settle -> try_send(RawSample) to the sampler task
sampler task:      redact -> size-cap -> batch (50 samples or 250 ms) -> sample service: one INSERT transaction -> metrics
sample service:    on its own connection and thread; a failed write drops the batch, counts it, backs off
```

### 4.2 Settings and runtime state across a config swap

The effective policy of a route is computed at compile time (global, then subsystem override, then route override)
and stored in the snapshot as an immutable `Arc<SamplingPolicy>`, so a request reads it with the snapshot it
already loaded (**a request loads the snapshot once**). The mutable parts (latency windows, the client 4xx table,
the token bucket) live outside the snapshot in a `SamplingRuntime` that compile receives, exactly
like the endpoint `RuntimeRegistry`: carried across swaps by key, never held in config, never in a global map.

### 4.3 Several instances on one database

```
instance A serves a slow request  -> sampler -> batch -> INSERT sample(instance='a', ...)   (no revision, no reload)
instance B serves a 502           -> sampler -> batch -> INSERT sample(instance='b', ...)
operator opens the Samples page on B -> SELECT ... ORDER BY id DESC -> sees both, each naming its instance
every instance, every 30 s        -> prune by age, rows, bytes in batches (idempotent, no leader)
operator changes the baseline rate on A -> settings write (revision, audit) -> B follows by the sync poll
```

What is per instance: the rolling p99 windows, the client 4xx table, the rate limiter, the body budget. What is
shared: the settings, the overrides and every sample. With the later PostgreSQL project the table moves with
the rest (portable types, a bitset instead of a JSON filter, database-assigned ids).

## 5. Configuration and data model changes

### 5.1 Config file (`config.toml`) and command line

```toml
[sampling]
enabled = true              # the master switch; false removes every capture cost (restart)
body_budget_bytes = "16MiB" # most memory teed bodies may hold at once, per process
queue_depth = 1024          # the bounded hand-off channel between the request path and the sampler task
batch_size = 50             # samples per database transaction, at most
batch_interval = "250ms"    # the longest a kept sample waits for its batch
```

Flags: `--sampling` and `--no-sampling` override `enabled`. Validation: sizes and counts have a minimum and a
maximum, unknown keys rejected as before, no secret read from the file. Retention (age, rows, bytes) is **not**
here: it is shared by every instance, so it lives in the database settings (5.2) and changes live.

### 5.2 Database (schema 4 to 5, edited in place; no migration)

```sql
CREATE TABLE sampling_settings (
  id                    INTEGER PRIMARY KEY CHECK (id = 1),
  enabled               INTEGER NOT NULL DEFAULT 1,
  triggers              TEXT NOT NULL,            -- JSON: which triggers are on
  slow_threshold_ms     INTEGER,                  -- NULL turns the static trigger off
  slow_p99_factor_x10   INTEGER,                  -- 30 = 3.0 times the rolling p99; NULL = off
  slow_p99_floor_ms     INTEGER NOT NULL DEFAULT 50,
  client_errors_per_min INTEGER,                  -- NULL = off
  oversized_request     INTEGER,                  -- bytes; NULL = off
  oversized_response    INTEGER,
  baseline_per_mille    INTEGER NOT NULL DEFAULT 1,   -- 1 = 0.1%, 0 = off
  rate_per_sec          INTEGER NOT NULL DEFAULT 20,
  rate_burst            INTEGER NOT NULL DEFAULT 40,
  storm_burst           INTEGER NOT NULL DEFAULT 3,       -- the storm guard (J19): per key
  storm_interval_secs   INTEGER NOT NULL DEFAULT 30,
  retention_days        INTEGER NOT NULL DEFAULT 7,       -- the shared sample table's bounds (J18)
  max_rows              INTEGER NOT NULL DEFAULT 100000,
  max_bytes             INTEGER NOT NULL DEFAULT 268435456,
  body_request_cap      INTEGER NOT NULL DEFAULT 4096,
  body_response_cap     INTEGER NOT NULL DEFAULT 4096,
  extra_headers         TEXT NOT NULL DEFAULT '[]',   -- additional selected request/response headers
  redact_headers        TEXT NOT NULL DEFAULT '[]',   -- additions only; built-ins are not stored
  redact_query_params   TEXT NOT NULL DEFAULT '[]',
  redact_body_fields    TEXT NOT NULL DEFAULT '[]',
  updated_by            TEXT NOT NULL,
  updated_at            INTEGER NOT NULL
);

-- One row per overridden target; removed with its subsystem or route. NULL = inherit.
CREATE TABLE sampling_override (
  id                  INTEGER PRIMARY KEY,
  scope               TEXT NOT NULL,                -- subsystem | route
  subsystem_id        INTEGER REFERENCES subsystem(id) ON DELETE CASCADE,
  route_id            INTEGER REFERENCES route(id) ON DELETE CASCADE,
  enabled             INTEGER,
  capture_bodies      INTEGER,
  baseline_per_mille  INTEGER,
  slow_threshold_ms   INTEGER,
  slow_p99_factor_x10 INTEGER,
  set_by              TEXT NOT NULL,
  set_at              INTEGER NOT NULL
);
CREATE UNIQUE INDEX sampling_override_target
  ON sampling_override (scope, COALESCE(subsystem_id, 0), COALESCE(route_id, 0));

-- One row per kept request, written by every instance. Not Config: no revision, no foreign keys (a sample
-- outlives its subsystem and route, and names are what the operator remembers). Portable SQL.
CREATE TABLE sample (
  id          INTEGER PRIMARY KEY,           -- assigned by the database: one increasing sequence for all writers
  at          INTEGER NOT NULL,              -- ms since the epoch, the time the response finished
  instance    TEXT    NOT NULL,
  request_id  TEXT    NOT NULL,
  triggers    INTEGER NOT NULL,              -- bit set over the fixed Trigger enum; filter with (triggers & mask) <> 0
  primary_trigger TEXT NOT NULL,
  subsystem   TEXT    NOT NULL,              -- a name, or _unmatched
  route       TEXT    NOT NULL,              -- a prefix, or _none
  client      TEXT    NOT NULL,              -- the peer address
  method      TEXT    NOT NULL,
  status      INTEGER NOT NULL,
  total_ms    INTEGER NOT NULL,              -- whole milliseconds, for the minimum-latency filter
  detail      TEXT    NOT NULL,              -- the redacted sample as JSON (headers, bodies, timings, attempts)
  size_bytes  INTEGER NOT NULL               -- length of detail plus the short columns, for max_bytes
);
CREATE INDEX sample_at        ON sample (at);
CREATE INDEX sample_subsystem ON sample (subsystem, id);
CREATE INDEX sample_status    ON sample (status, id);
CREATE INDEX sample_instance  ON sample (instance, id);
```

The settings row is created with its defaults the first time the schema is created. Redaction lists live only in
the global row, so no override can weaken them (J8). Lists read the short columns only (never `detail`), and a
sample's `detail` is parsed only for the detail and export views.

### 5.3 The sample, and the API (sketch)

```json
{
  "id": 4812, "instance": "a1b2", "requestId": "7f3c...", "at": "2026-11-12T09:41:07.312Z",
  "triggers": ["upstream_failure"], "primary": "upstream_failure",
  "subsystem": "shop", "route": "/api", "client": "203.0.113.9:51544",
  "request":  { "method": "POST", "path": "/api/orders", "query": "token=[redacted]&page=2",
                "headers": [["content-type", "application/json"], ["authorization", "[redacted]"]],
                "bytes": 812, "body": { "text": "{\"card\":\"[redacted]\"}", "truncated": true } },
  "response": { "status": 504, "headers": [], "bytes": 0, "body": null, "bodyOmitted": "off" },
  "timing":   { "totalMs": 30012.4, "upstreamMs": null },
  "upstream": { "endpoint": "10.0.0.5:8080", "attempts": [
                { "endpoint": "10.0.0.4:8080", "failure": "connect_refused" },
                { "endpoint": "10.0.0.5:8080", "failure": "response_timeout" } ] },
  "suppressed": 4812, "suppressedSinceMs": 60000
}
```

| Route | Action | Notes |
|---|---|---|
| `GET /api/sampling` | Read | settings, overrides, the table's size and the counters |
| `PUT /api/sampling` | Write | the global settings; field-specific validation errors |
| `PUT /api/sampling/overrides` | Write | set one override (target in the body); `DELETE` clears one |
| `GET /api/samples` | Samples | filters: `trigger`, `statusClass`, `subsystem`, `route`, `instance`, `client`, `minMs`, `since`, `before` (cursor = a sample id), `limit`; samples of every instance |
| `GET /api/samples/{id}` | Samples | one sample |
| `GET /api/samples/export` | Samples | NDJSON, same filters; audited |
| `DELETE /api/samples` | Samples | empties the table for every instance, in batches; audited |

A WebSocket event carries the capture and drop counts in the traffic snapshot (not each sample); a
`SamplingChanged` event follows a settings change.

## 6. Tasks

Task IDs are referenced by the dependency table in section 7. Workstream letters: **G** groundwork, **MD** model,
**S** store, **SM** the sampling crate, **SV** the sample service, **CS** config service, **R** routing and compile, **D** data plane,
**A** admin API, **U** admin UI, **Q** quality, **W** documentation, **C** carry-over.

### Workstream G: Groundwork

- [x] **G1 (S) Decisions and ADRs.** Resolve the **(confirm)** items in section 3. Write `docs/adr/`: 0020 the
  sampler: tail-based decision, triggers, rolling p99 and client counting (J1, J4-J7, J13), 0021 redaction and body
  capture (J8, J9, J15), 0022 samples in the shared database: the write path outside `ConfigService`, batching, retention and the
  cluster stance (J10, J11, J17, J18), 0023
  sampling settings outside `Config` and the access rule (J3, J12). Add the decisions to the ROADMAP log.
  *Done when:* the ADRs are merged and no **(confirm)** item is open.
  *Status:* done in PR1 (`m6/pr1-decisions`): the four ADRs are accepted; the owner confirmed every **(confirm)** item on
  2026-11-11, with J15 and J17 still subject to the numbers of spike G2.
- [ ] **G2 (S) Spike: the cost of the request head and of a body tee.** A throwaway benchmark against the
  `bench_backend` example: (a) retaining the selected headers (J15) versus a full `HeaderMap` clone versus
  nothing, per request; (b) a capped tee on `BodyOwner` and on the tracked response body, including a retried
  request (no double capture) and a body larger than the cap; (c) a `try_send` of a small struct per request; (d) **database contention**: configuration-write latency
  (`ConfigService::apply`) while the sample writer inserts batches at several times the default rate, with the
  writer on its own connection and thread, from one process and from **two processes on one database file**,
  and the pruner running; also the read latency of the list query on a full table (100 000 rows).
  Record findings in ADR 0021 and ADR 0022. Fallback if (a) costs more than about 1%: keep the head only when a request-time
  flag or the baseline draw (decided up front, since it does not depend on the outcome) says so, and for other
  triggers keep only the method, path and the five cheapest headers. Fallback if (d) shows configuration writes
  slowing noticeably: smaller or less frequent batches, a lower default rate, or a separate database file for
  samples (same code, a second path), at the price of the cluster sharing two files.
  *Done when:* the numbers and the chosen approach are in the ADRs and J15 and J17 are confirmed or changed.
- [x] **G3 (S) Defaults table.** Turn J4, J7, J8, J9, J10 and J19 into one documented defaults table (the single
  source for validation and the UI), including the built-in redaction lists and the unusual-header limits.
  *Landed in PR1:* the `Defaults` tables of ADRs 0020 to 0023.

### Workstream MD: Model

- [ ] **MD1 (M) Sampling settings types.** `SamplingSettings`, `SamplingOverride`, `SamplingTarget`,
  `Trigger` (fixed enum, stable names), `RedactionAdditions`, and validation with field-specific messages
  (`baselinePerMille` 0 to 1000, `ratePerSec` 1 to 10000, `stormBurst` 1 to 100, `stormIntervalSecs` 1 to 3600, `bodyRequestCap` up to 65536, `retentionDays` 1 to 365, `maxRows` and `maxBytes` within fixed bounds, list sizes and name
  syntax). Pure `effective(global, subsystem, route)` resolution.
  *Done when:* table-driven tests accept and reject every bound, and resolution order (route over subsystem over
  global, NULL inherits) and "overrides cannot remove redaction" are covered.
- [ ] **MD2 (S) The access vocabulary.** `Action::Samples`, the permission table (`Viewer` denied), the test that
  enumerates every role and action pair.

### Workstream S: Store

- [ ] **S1 (S) Schema.** Edit `schema.sql` in place with 5.2; bump `SCHEMA_VERSION` to 5.
- [ ] **S2 (M) Repositories.** `repository/sampling.rs`: read and write the settings row and the overrides as
  opaque rows (the store never interprets them); cascade on subsystem or route delete.
  *Done when:* round-trip, cascade and unique-target tests pass, and the schema test lists the new tables.
- [ ] **S4 (M) The sample repository.** `repository/samples.rs`: batch insert in one transaction, filtered list
  with a cursor over the short columns (trigger bit mask, status class, subsystem, route, instance, client,
  minimum latency, time), one sample's `detail`, a paged scan for export, the aggregate (rows, bytes, oldest) for
  retention, batched delete (oldest first, by age, by rows, by bytes), and clear. No revision is created and the
  cluster generation is untouched. The store keeps no knowledge of what a sample means.
  *Done when:* tests cover every filter, the cursor with concurrent inserts, a batch being atomic, each delete
  rule, that an insert does not change the revision version, and the manifest test still shows no crypto
  dependency.
- [ ] **S5 (S) A second connection for the sample service.** `Database` opens an additional connection (WAL, the
  same busy timeout) for the sample writer thread, so sample writes never wait in the store thread's queue; the
  config store thread and its `Database::write` are unchanged.
  *Done when:* a test shows a long sample batch and a configuration write both completing, and a held write lock
  producing a counted error rather than a hang.
- [ ] **S3 (S) Revision interplay.** Summary-only revisions for sampling changes (like maintenance), never in the
  configuration copy.
  *Done when:* a test shows a rollback leaves the settings and overrides unchanged.

### Workstream SM: The sampling crate (new)

- [ ] **SM1 (S) Crate and vocabulary.** `fast-proxy-sampling` with `Trigger`, `SamplingPolicy` (the compiled
  effective policy), `RequestHead`, `Outcome`, `RawSample`, `Sample`; depends on `model` only.
- [ ] **SM2 (M) The decision.** `decide(policy, head, outcome, state, now_ms) -> Triggers` as one pure function
  over atomics: every trigger of J4, the priority order, the baseline draw (injectable random source), the
  maintenance and unmatched-host exemptions of J13, and an upstream-answered 5xx matching nothing.
  *Done when:* a table test covers every trigger alone, in combination, at the exact threshold, disabled, and
  with an override; an upstream 500 and an upstream 502 produce no trigger; every connectivity failure kind
  produces `upstream_failure`; a retry after a connection refused produces `retried` and a retry after an
  upstream 503 does not.
- [ ] **SM10 (M) `StormGuard`.** The per-key limiter of J19: the 1024-slot table, the key from subsystem, route,
  trigger and (for connectivity triggers) endpoint and failure kind, the burst and interval from the policy, the
  suppressed counter and its hand-over to the next kept sample, the clock as a parameter, no allocation.
  *Done when:* tests cover the burst then the interval, one key not affecting another, the suppressed count
  arriving on the next kept sample exactly once, a quiet period restoring the burst, a collision merging two keys
  without panic, concurrent callers never exceeding the budget, and the `baseline` exemption.
- [ ] **SM3 (M) `LatencyWindow` and the p99 publisher.** The two-epoch log-linear histogram, the quantile
  computation, the published atomic threshold, the minimum-count rule; the 5-second tick as a task that takes
  `now_ms`.
  *Done when:* property tests show the estimate within one bucket of the true p99 on random data, an epoch roll
  forgets old latencies, and recording under concurrency loses nothing.
- [ ] **SM4 (S) `ClientErrors`.** The 4096-slot windowed table and the "sample the crossing, then one in N" rule
  (J6).
  *Done when:* tests cover the crossing, the window roll, a collision firing early without panicking, and zero
  allocation after construction.
- [ ] **SM5 (S) `RateLimiter`.** A lock-free token bucket (the clock is a parameter).
  *Done when:* tests cover burst, refill, a zero-rate setting and concurrent callers never exceeding the budget.
- [ ] **SM6 (M) The redactor.** Headers, query strings and the lexical body redactor of J8, including truncated
  JSON and form bodies and an unredactable tail being dropped.
  *Done when:* table tests cover each built-in name case-insensitively, nested JSON, escaped quotes, a cut in the
  middle of a string or key, arrays, form encoding with percent escapes, and additions; a property test asserts
  **no input containing a known secret value after a redacted name ever yields output containing it**.
- [ ] **SM7 (M) The sample encoding and the query model.** `Sample` to its short columns and its `detail` JSON and
  back, the size accounting (`size_bytes` equals the stored lengths), the `SampleQuery` filter type with its
  validation and the trigger bit set, and an NDJSON writer over rows.
  *Done when:* round-trip tests cover every field, a sample at every cap, unusual bytes, and a filter mask for
  every trigger.
- [ ] **SM8 (S) The batcher and the sink trait.** `SampleSink` (an async-free trait the sample service implements)
  and the pure batching rule (flush at `batch_size` or after `batch_interval`, whichever first), with the clock as
  a parameter.
  *Done when:* tests cover both flush conditions, an empty interval writing nothing, and order being preserved.
- [ ] **SM9 (M) The sampler task and handle.** `Sampler` (the cheap handle the data plane holds: policy lookup
  helper, `try_send`, the body budget counter) and the task that redacts, size-caps, batches and passes batches to
  the `SampleSink`, counting what the sink reports. Shutdown flushes the queue and the last batch (like
  `AccessLog::flush`).
  *Done when:* tests (with a fake sink) cover a full queue dropping and counting, a failing sink dropping the batch
  and counting `store_error`, the budget being released on every path (kept, dropped, not triggered), and
  shutdown losing nothing already accepted.

### Workstream SV: The sample service (new crate `fast-proxy-sample-service`)

- [ ] **SV1 (M) The writer.** Implements `SampleSink` over the second connection (S5): one transaction per
  batch, a busy timeout, a backoff after a failure (no unbounded retry, no unbounded memory while it backs off),
  the instance id stamped on every row, and metrics hooks for written rows and failures.
  *Done when:* tests cover a batch landing atomically, a locked database dropping and counting, recovery after the
  lock clears, two writers (two threads, and two processes in the integration test) producing one increasing id
  sequence, and shutdown flushing.
- [ ] **SV2 (M) The retention pruner.** Every instance runs it every 30 seconds, reads the live retention
  settings, and deletes oldest-first in batches of 500 (age, then rows, then bytes), yielding between batches;
  idempotent and leaderless; a failed pass is counted and retried next tick.
  *Done when:* tests cover each limit alone and together, two pruners racing without error or over-deleting by
  more than a batch, a settings change taking effect on the next pass, and a 200 000-row prune never holding the
  write lock for more than a small bounded time.
- [ ] **SV3 (M) The reader.** The queries the admin plane calls (list with filters and cursor, detail, paged
  export, clear, the status aggregate), on a read connection, returning typed samples.
  *Done when:* tests cover every filter against rows from two instances, the cursor being stable while another
  writer inserts, and the list never reading `detail`.

### Workstream CS: Config service

- [ ] **CS1 (M) Writes with audit.** `set_sampling`, `set_sampling_override`, `clear_sampling_override`, through
  `ConfigService::apply` with a `ChangeContext` and `Audited` descriptions (`sampling.update`,
  `sampling.override.set`, `sampling.override.clear`); validation by compiling the would-be policy; the audit
  detail holds names and numbers only.
  *Done when:* tests cover the revision, the audit row in the same transaction, a vetoed write leaving nothing,
  and the sync poll carrying a change to a second service.
- [ ] **CS2 (S) Loading.** The settings and overrides are read with the rest of the state at start and on every
  reload and sync, and handed to compile; the `SamplingChanged` broadcast.

### Workstream R: Routing and compile

- [ ] **R1 (M) Compile the effective policy.** `SamplingInputs` (beside `Config`, like `MaintenanceInputs`) and a
  `CompiledSampling` per route (and for the unmatched case), holding the `Arc<SamplingPolicy>` and the route's
  runtime handles from the `SamplingRuntime`.
  *Done when:* tests cover inheritance, a route deleted and recreated, and the runtime state surviving a swap by
  key while a removed route's state is dropped.
- [ ] **R2 (S) Snapshot access.** The accessors the data plane uses, with no allocation and no clock read when the
  route's sampling is off.

### Workstream D: Data plane

- [ ] **D1 (M) `RequestHead` capture.** In the pipeline after the route matched: the unusual-header flags (J7),
  the selected headers, method and the path and query, stored in the `PendingRequest`; nothing at all when the
  policy for the route is off. The maintenance bypass header is never in the selected set.
  *Done when:* tests cover each flag, the allow-list, and that a disabled route allocates nothing (a counting
  allocator test or the benchmark).
- [ ] **D2 (M) The request body tee.** On `BodyOwner`: capped capture for opted-in routes, the budget
  reservation, content-type and encoding rules, never for upgrades, never twice across attempts.
  *Done when:* tests cover the cap, a body that is exactly the cap and one byte over, a binary content type, a
  gzip body, a budget at zero, a retried request, and the bytes-in count (also used by `oversized_request`).
- [ ] **D3 (M) The response side and the decision.** The response header selection and an optional capped tee in
  `TrackedBody`; the decision at drop and in `PendingRequest::complete`; `LatencyWindow.record`; the storm guard, the token
  bucket and the hand-off; each attempt's endpoint and failure kind taken from the upstream client's result. A cancelled request (499) follows J13.
  *Done when:* end-to-end tests cover each trigger against a real backend (a backend that is killed, refuses, hangs past the timeout, resets
  the connection, slows, returns its own 500 and 502 (no sample), recovers on retry, sends a large response), a client that leaves early, a maintenance 503 not being sampled, and the access log
  line being unchanged.
- [ ] **D4 (S) Metrics.** `fastproxy_samples_captured_total{trigger}` (kept by the decision),
  `fastproxy_samples_dropped_total{reason}` (`storm_guard`, `rate_limit`, `queue_full`, `budget`, `store_error`),
  `fastproxy_samples_written_total`, `fastproxy_sample_queue_depth`, `fastproxy_sample_write_seconds`
  (histogram), `fastproxy_sample_prune_deleted_total`, `fastproxy_sample_table_rows` and
  `fastproxy_sample_table_bytes` (set by the pruner from its aggregate, per instance reading of the shared table),
  `fastproxy_sample_body_budget_bytes`. Labels are fixed enums only.
  *Done when:* a test shows no label can come from a request value, and the series render.
- [ ] **D5 (S) Startup wiring.** `startup.rs` builds the `SamplingRuntime`, the sampler task and its shutdown, and
  passes the handle to the pipeline through `Observability`, starts the sample service (writer and pruner) on the
  second connection and stops it after the data plane drains; `[sampling] enabled = false` builds nothing.

### Workstream A: Admin API

- [ ] **A1 (M) Settings endpoints.** `api/sampling.rs`: get, put, override set and clear, with the shared error
  envelope and field errors; registered in `RouteTable` with their `Access`.
  *Done when:* tests cover validation errors per field, a viewer's 403 on writes and 200 on read, and the audit
  entries.
- [ ] **A2 (M) Sample endpoints.** `api/samples.rs`: list with filters and a cursor, detail, the streamed NDJSON
  export (read from the table in pages, never loaded whole), clear (every instance's samples); `Access::Needs(Action::Samples)`; the export and clear audited.
  *Done when:* tests cover every filter (including `instance`), a viewer's 403 on all of them, the export being valid
  NDJSON that equals the list, a list from instance B showing samples written by instance A, and the audit rows.
- [ ] **A3 (S) Events and status.** The capture and drop counts in the traffic snapshot, the `SamplingChanged`
  event, the table status in `GET /api/sampling`, and the WebSocket documentation in `events.rs`.

### Workstream CL: CLI (small)

- [ ] **CL1 (S) `samples` subcommand.** `fast-proxy samples list|export` through the admin API (same token,
  URL and CA options as the other subcommands), NDJSON to stdout; exit codes as in M5.
  *Done when:* a CLI test lists and exports from a running proxy and refuses with exit code 1 for a viewer token.

### Workstream U: Admin UI

- [ ] **U1 (M) API client and types.** `lib/types.ts`, `lib/api.ts` functions, camelCase validation in
  `lib/validation.ts` (caps, rates, name lists), the new event in `LiveContext.tsx`.
- [ ] **U2 (L) Samples page.** `pages/Samples.tsx`: a filterable table (trigger chips, status class, subsystem,
  route, minimum latency, time range), a live-tail toggle that polls with the cursor, a detail drawer (headers,
  bodies with a "truncated" or "omitted: reason" note, timings, attempts, the request id with a link to nothing
  else), export, clear with a confirmation. Admin only: a viewer sees an explanation and the status counters.
- [ ] **U3 (M) Sampling settings.** A Sampling section in Settings: master status, trigger switches and
  thresholds, the baseline rate and the rate limit with the plain meaning of each ("about 1 in 1000"), the
  redaction additions (the built-in lists shown read-only), the retention limits (age, rows, bytes) with the table's current size, and the drop counters.
- [ ] **U4 (M) Per-target overrides and the body opt-in.** A sampling control on the subsystem detail page and on
  each route: inherit, off, or custom, and an explicit "capture bodies" switch with a warning about what it holds.
- [ ] **U5 (S) Dashboard card and navigation.** Capture rate and drops on the dashboard, a "Samples" sidebar item
  (`navigation.ts`), the page title.
- [ ] **U6 (S) Tests.** Vitest page tests with `test/mockApi.ts` for U2 to U5, including the viewer view, and the
  CSP check of the built page still passing.

### Workstream Q: Quality

- [ ] **Q1 (L) Exit-criteria tests.** `tests/sampling.rs` against the real binary and real backends: each trigger
  of criterion 1 (the `flaky.rs` backend fails, slows, and sends large bodies), the negative case, the baseline
  at a 100% rate setting, overrides, a live settings change without dropping a connection, a restart, export and
  clear, the rate limit, and the storm guard (kill the only backend of a route, send 1,000 requests: at most the
  burst plus the interval's worth of samples exist, the next sample carries `suppressed`, and a `slow` request on
  another route during the outage is still kept).
- [ ] **Q2 (M) Secret hygiene.** Extend `tests/secret_hygiene.rs`: requests carrying a bearer token, a cookie, a
  bypass secret, secret query parameters and secret body fields; assert none appears in the API, the export, the
  database file and its WAL (read as bytes, as the M3 key tests do), the log, the metrics or the debug output of any type holding a sample.
- [ ] **Q3 (M) Sampling under load.** `tests/sampling_under_load.rs`: a keep-alive load (`load.rs`) in which a dead upstream makes
  every request a connectivity failure for a minute, then a mixed load with one route down; assert throughput, that no request fails or is delayed because of
  sampling, that the drop counters account for the difference between triggered and kept, memory (queue and body
  budget) and the table (retention) stay within their bounds, that a configuration write during the flood completes
  within a small multiple of its idle latency, and that shutdown flushes.
- [ ] **Q4 (S) Cluster.** `tests/cluster_operations.rs` gains: a settings change on one instance reaching the
  other within the poll, a sample served by A being listed by B (and the reverse) with its instance named, samples
  surviving a restart of the instance that wrote them, a sample write not bumping the revision version or causing a
  reload on the other instance, `clear` on one instance emptying the table for both, and two processes pruning and
  inserting on one database file without errors.
- [ ] **Q5 (M) Property tests.** The redactor (Q2's property at scale), the retention bounds, the window estimate.
- [ ] **Q6 (S) Benchmark.** Re-run the `oha` baseline of [benchmarks.md](benchmarks.md): sampling off, on at
  defaults, on with body capture for the route, and the dead-upstream load; record the numbers against the budget of
  section 8.
- [ ] **Q7 (S) Mutation checks.** For each new test that passed first time, break the behavior it guards (skip a
  redaction, drop the rate limit, ignore the cap) and confirm it goes red, as the project rules ask.
- [ ] **Q8 (M) Security review.** `docs/security-review-m6.md`: what a sample can leak, the redaction coverage and
  its known limits (secrets in a field not on the list, in a path segment, in a free-text body), the access rule,
  memory and table bounds, who can read the table (the database file now holds redacted request data, so its
  permissions and backups matter), and the audit coverage; fix what it finds.
- [ ] **Q9 (S) The gates.** `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `cd ui && npm test`, `cd ui && npm run build`, and `cargo llvm-cov --workspace --summary-only`
  (overall above 60%, aim higher for the decision and redaction code); report the number. Tests must pass on
  Linux, macOS and Windows: no fixed short sleeps, `#[cfg(unix)]` for file-mode checks.

### Workstream W: Documentation

- [ ] **W1 (S)** The ADRs of G1 completed with the spike results and any change found while building.
- [ ] **W2 (S)** README and `docs/getting-started.md`: turning sampling on for a route, reading a sample with
  curl, what is redacted and what is not, the body opt-in warning. Keep the walkthrough working.
- [ ] **W3 (S)** AGENTS.md: the new crates (`fast-proxy-sampling`, `fast-proxy-sample-service`), the architecture rules (samples are written by the sample service, never by `ConfigService`, and never create a revision; the data plane still never touches the store),  the new tests, and the
  `[sampling]` configuration section. ROADMAP: tick M6, the decision-log entries, the risks table.
- [ ] **W4 (S)** [benchmarks.md](benchmarks.md) with the sampling numbers, and an operations note: sizing the
  table (retention), the queue and the body budget, reading the drop counters, what a database backup now
  contains (redacted request data), and what to do when the database is busy.

### Workstream C: Carry-over and housekeeping

- [ ] **C1 (S) The owner's check.** The M5 browser click-through and the M3/M5 stack merge are still open
  ([M5-PLAN.md](M5-PLAN.md) section 12); finish them before the UI tasks so M6 builds on a merged base.
- [ ] **C2 (S) Configuration import/export** is still open from M3 and M5; decide whether it is taken here, in M7,
  or dropped. Not in M6's scope unless the owner says so.

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| G1 | none (needs the owner's answers) |
| G2 | none (a spike; can start now) |
| G3 | G1 |
| MD1, MD2 | G3 |
| S1 | MD1, MD2 |
| S2, S3 | S1 |
| SM1 | MD1 |
| SM2 | SM1 |
| SM3, SM4, SM5, SM6, SM10 | SM1 |
| SM7 | SM1, SM6 |
| SM8 | SM1 |
| SM9 | SM2-SM8, SM10 |
| S4 | S1 |
| S5 | S1 |
| SV1 | S4, S5, SM9 |
| SV2 | S4, S5, SV1 |
| SV3 | S4, SM7 |
| CS1 | S2, S3, MD1 |
| CS2 | CS1, R1 |
| R1 | MD1, SM1, SM3 |
| R2 | R1 |
| D1 | R2, G2 |
| D2 | D1, SM9 |
| D3 | D1, D2, SM2-SM5, SM9, SM10 |
| D4 | D3 |
| D5 | D3, CS2, SV1, SV2 |
| A1 | CS1, MD2 |
| A2 | SV3, MD2 |
| A3 | A1, D4 |
| CL1 | A2 |
| U1 | A1, A2, C1 |
| U2 | U1, A2 |
| U3 | U1, A1 |
| U4 | U1, A1 |
| U5 | U1, A3 |
| U6 | U2-U5 |
| Q1 | D1-D5, A1, A2 |
| Q2 | D3, A2, SV1 |
| Q3 | D3, D5 |
| Q4 | CS1, CS2, D5, SV1-SV3 |
| Q5 | SM3, SM6, SM7 |
| Q6 | D5 |
| Q7 | every code task |
| Q8 | every code task |
| Q9 | every code task (runs in CI on each PR) |
| W1-W4 | feature tasks complete |
| C1, C2 | none |

Suggested slicing into pull requests, each leaving `main` green and releasable. **Merge each to `main` as it
passes CI** so the stack stays short:

1. **PR1** G1, G3 (decisions, ADRs 0020 to 0023, defaults); G2 as its own throwaway branch.
2. **PR2** MD1, MD2, S1-S5 (model types, schema including the `sample` table, repositories, the second
   connection, the access action): no behavior yet.
3. **PR3** SM1-SM9 (the sampling crate, complete and tested in isolation): no behavior yet.
4. **PR4** CS1, CS2, R1, R2 and SV1-SV3 (settings writes, audit, compile, runtime state across swaps; the sample
   service with its writer, pruner and reader, tested against a real database).
5. **PR5** D1-D5, A3 (capture end to end, metrics, wiring, events).
6. **PR6** A1, A2, CL1 (the settings and sample APIs and the CLI).
7. **PR7** U1-U6 (the UI), after the owner's C1 click-through.
8. **PR8** Q1-Q9, W1-W4 (exit-criteria, hygiene and load tests, the benchmark, the security review, docs and the
   outcome section).

PR3 is independent of PR2 after the model types and can be taken in either order; PR6 can start once PR4 lands.

## 8. Performance and cost budget

- **Sampling on at defaults** must stay within about 2% of the M5 numbers in [benchmarks.md](benchmarks.md)
  (one endpoint, 64 connections). The per-request cost is: one snapshot field read, the selected-header clones
  (J15, spike G2), one atomic increment for the latency window, a few integer comparisons, and no allocation for
  a request that is not sampled.
- **Every request is a connectivity failure** (the only backend of a route is down, sampling at defaults):
  throughput within about 5% of the same load with sampling off, and the table gains only the storm guard's
  allowance (about the burst plus one sample per interval per failing endpoint). The storm guard and the token
  bucket are checked **before** any sample is built, so the cost of a suppressed sample is one atomic.
- **Body capture on a route** costs one extra copy of at most the cap per request on that route only; measure and
  record it (it is opt-in, so it is a documented cost, not a gate).
- **The sampler task** keeps up with the rate limit by a wide margin (redaction of a 4 KiB body in microseconds);
  a burst beyond the queue depth is dropped, never queued without bound.
- **The database is not on the request path** and must not slow configuration writes: batches of about 50 rows
  commit in a few milliseconds, the writer has its own connection and thread, and spike G2 (d) sets the budget;
  the figure to hold is configuration-write latency during a sample flood within about 2x its idle value. The
  list query on a full table (100 000 rows, indexed short columns) should answer in well under 100 ms; the
  pruner deletes 500 rows per statement and yields between them.
- **Memory is bounded** by `body_budget_bytes` plus the queue (`queue_depth` times the
  per-sample cap) plus one batch plus the fixed tables (latency windows are 64 atomics per route, the client table is fixed).
  Record the figure.
- **Reload** with sampling settings compiles per-route policies in the same pass as routes; a reload with
  thousands of routes must stay within the M5 figure plus a small constant.

## 9. Risks specific to M6

| Risk | Mitigation |
|---|---|
| **Secrets leaking into samples** (headers, query strings, bodies, or fields nobody listed) | Redaction is built in and always on, applied once before anything is stored; additions only; bodies off by default and per route; text content types only; an unredactable tail is dropped; Q2's end-to-end scan of every output and Q5's property test; the security review states the residual risk (a secret in an unlisted field, in a path segment, or in free text) plainly, and the UI and docs warn at the body opt-in |
| **Latency or throughput cost on the hot path** | Decision is integer comparisons; no allocation unless a sample is kept; the rate limit is checked first; the hand-off is a non-blocking `try_send`; the master switch removes everything; spike G2 chooses the request-head approach with numbers, and Q6 measures the budget |
| **Memory exhaustion** (large bodies, a flood of samples) | Per-sample caps, a ring bounded by count and bytes, a process-wide body budget with a "budget" omission instead of blocking, a bounded queue, a fixed-size client table; Q3 asserts the bounds under load |
| **A tee changing proxy behavior** (a retry replaying a half-read body, backpressure altered, a slower response) | The tee observes frames and never holds a request back; it sits on the body owner so a replayed attempt cannot be captured twice (ADR 0008 rule: a body is replayable only if never polled); the existing retry and body tests run with capture on; G2 tests it before the design is fixed |
| **Redaction cannot parse a truncated body** | A lexical redactor designed for prefixes, not a parser; anything uncertain is dropped; property tests on random cuts |
| **An outage flooding the table with identical samples** (every request to a dead endpoint is a connectivity failure) | The storm guard (J19): a burst of 3 then one per 30 s per route, trigger, endpoint and failure kind, a suppressed count carried on the next sample so the size of the outage is still visible, no draw on the global budget so other triggers keep working, then retention; asserted by an end-to-end outage test (Q1) and the dead-upstream load (Q3) |
| **A flood making the table useless** (one scanner, one failing route filling it) | The per-instance token bucket, "the crossing, then one in N" for a noisy client, retention by age, rows and bytes (the oldest go first, counted and visible), and the baseline sample keeps a comparison alive; per-route overrides can turn a noisy route off |
| **Rolling p99 wrong at low traffic or after a swap** | A minimum count and a floor before the relative trigger can fire; state carried across swaps by key; the trigger is off by default |
| **Cardinality** (client addresses, paths, or triggers becoming labels) | Trigger and reason names are fixed enums; no client address, host or path is ever a label (the existing rule, checked by a test in D4) |
| **Sample writes slowing configuration writes, or locking the database** (SQLite has one writer; several processes share the file) | Own connection and thread for samples, small short batches, a busy timeout, a failed batch dropped and counted rather than retried without bound; spike G2 (d) measures it across threads and processes before the design is final, with a separate database file as the documented fallback; Q3 and Q4 assert it |
| **A sample write mistaken for a configuration change** (a revision per sample would make every instance reload constantly) | J17: samples create no revision, no audit row and no generation bump; a test asserts the revision version is unchanged by a flood (S4, Q4) |
| **The table growing without bound with many instances** | Retention by age, rows and bytes from shared settings, pruned by every instance in batches (idempotent, no leader); the per-instance rate limit bounds the inflow; the table and queue sizes are metrics |
| **Request data now sits in the database file** (copies, backups and replicas carry it) | Redaction happens before the write, so what is stored is already redacted; bodies are off by default; retention defaults to 7 days; `GET /api/samples` is admin-only; the docs and the security review (Q8) say what a backup contains; the clear action deletes in every instance's view |
| **A sample lost when a process dies** | At most one batch interval (250 ms) of kept samples; stated in the docs; the access log still has the request |
| **Per-instance state surprising operators** (a p99 trigger that differs between instances, a rate limit that multiplies by instance count) | Documented in the UI and ADR 0022: the windows, client table, rate limit and body budget are per instance; retention bounds the cluster total |
| **The new access rule is too strict or too loose** | A new `Action::Samples` kept in the one permission table with its enumerating test; the status is readable by a viewer, the content is not; export and clear audited |
| **Samples taking the wrong meaning for maintenance** (a planned 503 treated as an incident) | J13: maintenance answers are never unusual by themselves; tested |
| **Scope creep** (expression rules, external shipping, replay, cross-instance views) | Section 2 lists what is out; new ideas go to the roadmap, not this plan |

## 10. Definition of done for M6

- [ ] Every task above is checked, or explicitly moved with a note.
- [ ] Exit criteria (section 1) demonstrated by the end-to-end tests in Q1 to Q4, with the table in the outcome
  section. The manual run (turn sampling on for one route, trigger each case, see the redaction, opt a route into
  bodies, export, change a setting live, restart) is the owner's click-through.
- [ ] CI green on all three platforms; no new clippy warnings.
- [ ] Overall test coverage above 60%, the number recorded here.
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [ ] README, AGENTS.md, ROADMAP.md and getting-started updated.
- [ ] Benchmark re-run and recorded within the budget of section 8.
- [ ] The security review (Q8) done, findings recorded, secret hygiene verified (Q2).

## 11. What M6 leaves for M7, and other touch points

- **Request limits are M7's.** The unusual-header limits of J7 are constants that sample, never refuse. M7's
  header-size defenses and connection limits will refuse; when they land they should reuse the same limits table
  so a refused request and a sampled one agree.
- **Rate limiting** (M7) will produce 429s; the `client_errors` trigger will see them as 4xx, so a rate-limited
  scanner is sampled at the capped rate. Nothing in M6 needs to change.
- **Fuzzing** (M7) gets two new targets for free: the redactor and the request-head flag scan, both pure.
- **ACME (M4)** adds a challenge path answered ahead of routing; it is not a routed request, so it is not
  sampled. No touch point.
- **M5 audit** gains the actions `sampling.update`, `sampling.override.set`, `sampling.override.clear`,
  `samples.export` and `samples.clear`; the audit code is unchanged.
- **M5 maintenance** and M6 meet in one rule (J13) and in the redaction of the bypass header name; both are tested.
- **M3 cluster sync** carries the settings and overrides (summary-only revisions); samples are **not** part of it:
  they are plain rows in the shared table, read by every instance directly and never a reason to reload.
- **The PostgreSQL project** takes the `sample` table with the rest: the columns use portable types, the trigger
  filter is a bit mask, ids come from the database, and the writer, pruner and reader sit behind the repository
  so only the SQL dialect changes; at very high sampling rates a dedicated store becomes the better home
  (out of scope here).

## 12. Outcome

*To be written when M6 ships.*
