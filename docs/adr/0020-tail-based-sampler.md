# ADR 0020: the tail-based sampler, its triggers and its storm guard

Status: accepted by the owner (2026-11-11). Plan reference: M6 J1, J4 to J7, J13, J19.

## Context

An operator who sees a burst of failures or a slow route has metrics and an access log
line, but not what the request looked like. Capturing everything is too costly and too
risky. A sampler must choose after the response, when the status, the latency and the
attempts are known, and must never slow or block a request.

## Decision

- **The decision runs when the response has finished**, where the request is already
  reported (`TrackedBody::drop`, and `PendingRequest::complete` for a client that left
  first). The request side only contributes facts: a compact `RequestHead` (method, path
  and query, a few selected headers, flags) taken before forwarding.
- **Triggers.** A sample records every trigger that matched (a list; the first in the
  order below is the primary): `upstream_failure`, `retried`, `slow`, `slow_p99`,
  `client_errors`, `oversized_request`, `oversized_response`, `unusual_headers`,
  `baseline`. The names are a fixed enum, so they are safe metric labels.
- **A 5xx that an upstream answered is not a trigger.** It is the application's own
  answer, and the metrics and the access log already count it. `upstream_failure` means a
  **connectivity** failure ended the request: connect refused or timed out, the upstream
  TLS handshake failed, a connection reset or closed before a response head, or a
  response timeout, answered by the proxy's own 502 or 504. The proxy's own answer is told
  apart from an upstream's by the error kind the upstream client reports
  (`upstream_errors.rs`), never by the status. `retried` means a connectivity failure on
  one attempt was retried and the request then got an answer; a retry caused only by an
  upstream 5xx (ADR 0008) is not a sample.
- **Rolling p99.** A per-route `LatencyWindow`: a log-linear histogram of atomics over two
  one-minute epochs; a tick every 5 seconds publishes the p99 as one atomic, so the
  request path reads an integer. Keyed by `(subsystem name, route prefix)` and carried
  across snapshot swaps like endpoint state (ADR 0005). Only after 200 requests in the
  window, with a floor, and off by default.
- **Per-client 4xx counting.** A fixed table of 4096 slots indexed by a hash of the client
  address, one window per slot. A collision can only fire a trigger early. The crossing
  request is sampled, then one in N while the client stays above the threshold.
- **Unusual headers** are decided at request time from facts that need no allocation
  (more than 64 headers, a value over 8 KiB, a non-visible-ASCII or invalid UTF-8 value, a
  duplicated singleton header, `Content-Length` with `Transfer-Encoding`, an unknown
  method, a path over 8 KiB). The limits are constants that sample and never refuse
  (request limits are M7).
- **Never unusual by themselves** (J13): a maintenance answer (it contacts no upstream),
  the fixed unmatched-host 404, a redirect. A bypassed request is sampled like any other.
  A client that left (499) is sampled only when another trigger matched or by the baseline.
- **The storm guard** (J19). A connectivity failure can fire for every request at once, so
  before the global token bucket each candidate passes a per-key limiter. The key is
  `(subsystem, route, primary trigger)` and, for `upstream_failure` and `retried`, also the
  endpoint address and failure kind, so a dead endpoint is one key. A key gets a burst of
  3, then one per 30 seconds. Suppressed candidates are counted per key and the count is
  carried on the next kept sample (`suppressed`, `suppressedSinceMs`); they count as
  `dropped{reason="storm_guard"}` and take nothing from the global bucket, so other
  triggers keep working. The key table is fixed size (1024 slots, atomics, no allocation);
  `baseline` is exempt. The guard is per instance.
- **The decision is one pure function** (`decide`) in `fast-proxy-sampling`, over atomics,
  with the clock and the random source as parameters.

## Defaults

| Setting | Default |
|---|---|
| `upstream_failure`, `retried` | on |
| `slow` (static) | on, 2000 ms |
| `slow_p99` | off; factor 3, floor 50 ms, 200 requests minimum |
| `client_errors` | on, 100 4xx per client per minute |
| `oversized_request`, `oversized_response` | on, 8 MiB and 32 MiB |
| `unusual_headers` | on |
| `baseline` | on, 1 in 1000 |
| storm guard | burst 3, then 1 per 30 s per key |

## Consequences

- The request path does a snapshot read, a few integer comparisons and one atomic for the
  latency window; nothing is allocated for a request that is not sampled.
- An outage produces a handful of samples per failing endpoint per minute and a count of
  the rest, instead of filling the table.
- Latency windows, the client table and the storm guard are per instance and are not in
  the database (ADR 0022).

## Found while building (M6 PR8)

The token bucket shared by the storm guard and the rate limiter stored the low 32 bits of a
millisecond clock. A caller reads the clock before it reaches the bucket, so under concurrency
a reading a few milliseconds older than the stored one looked like a gap of 49 days and
refilled the bucket. The bucket now treats a reading less than a minute behind as a late
arrival: no credit, stored time unchanged. The design (burst, then one per interval, suppressed
count carried forward) is unchanged; property tests feed both limiters out-of-order readings.
