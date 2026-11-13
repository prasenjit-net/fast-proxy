# Security review of M6 (request sampling)

Task Q8 of [M6-PLAN.md](M6-PLAN.md): the checklist run before the release, with how each
item was checked and what was found. Date: 2026-11-13. Scope: everything M6 added: the
sampling decision, the redactor and body capture, the queue and the writer, the pruner, the
settings and overrides, the sample API and its export, the CLI commands and the UI. The M3
and M5 reviews still hold for certificates, keys, tokens and the admin plane.

The threat is not an attacker on the wire; it is **what the proxy now keeps**. A sample is a
copy of a real request, made because it was unusual, and unusual requests are exactly the
ones that carry malformed input, scanners' payloads and, now and then, someone's credentials.

## What a sample can hold

| Part | Kept | Redacted | Not kept |
|---|---|---|---|
| Method, path, client address, status, timings, endpoint and failure kind | always | no | |
| Query string | always | the values of built-in and operator-listed parameter names, and any name containing `token`, `secret`, `password` or `passwd` | |
| Request headers | a fixed set of ordinary ones (`Host`, `User-Agent`, `Content-Type`, `Referer`, `Origin`, ...) and those the operator lists | the value of a header that is built-in sensitive (`Authorization`, `Cookie`, `X-Api-Key`, ...), listed as sensitive, or a bypass header of a maintenance scope; the query string inside a `Referer` URL (finding 3) | every other header, even its name |
| Response headers | a fixed set (`Content-Type`, `Location`, `Retry-After`, ...) | as above (`Set-Cookie` is built-in sensitive); the query string inside `Location` | the rest |
| Bodies | only for a route that opts in, text types only (JSON, form, XML, plain text), cut at the cap (4 KiB by default), within a process-wide memory budget | members, fields and elements with sensitive names, in JSON, forms, XML and text, safe at any cut | binary types, everything when the route has not opted in |

Redaction happens **before** anything is queued for storage, so the queue, the database, its
write-ahead log, the API, the export and the metrics only ever see redacted values.

## Checklist

| # | Item | Result | How it was checked |
|---|---|---|---|
| 1 | No secret of a sampled request reaches the API, the export, the metrics, the database file or its WAL | pass | `tests/sampling_hygiene.rs` sends a request carrying a bearer token, a cookie, an API key, two secret query parameters (one built-in, one the operator added), secret body fields (built-in, nested, and operator-added) and a header the operator marked sensitive, with bodies on for the subsystem. It reads the list, the detail, the export and `/metrics`, then stops the proxy and scans `fast-proxy.db`, `-wal` and `-shm` as bytes. Ordinary values (an extra header, `user=ann`, the field `name`) are asserted present, so storing nothing cannot pass. Skipping the header, query or body redaction each turned it red |
| 2 | A header that is kept because the operator listed it is still redacted when sensitive | pass | The same test lists `authorization` and a custom secret header as headers to keep and asserts both stored as `[redacted]` |
| 3 | Bodies are off unless a route opts in | pass | `a_sample_without_body_capture_holds_no_body_at_all`; the data plane's `bodies_are_not_captured_unless_a_route_opts_in` counts that no byte is teed either |
| 4 | The redactor cannot be cut into a leak | pass | Property tests (`fast-proxy-sampling`, `property_tests.rs`): for generated JSON and form bodies and **every** cut point, no secret appears in the output; plus the example tests for XML, text and multi-byte characters |
| 5 | Only an admin can read a sample; a viewer sees counts | pass | `Action::Samples` is admin only; the route table test fails for a route without an `Access`; `samples_api_tests` cover 401, 403 and the viewer's `GET /api/sampling`; the UI shows a viewer an explanation and never requests a sample |
| 6 | Reading in bulk and deleting are audited | pass | `samples.export` and `samples.clear` are written through `record_audit`; `tests/sampling.rs` checks the clear is in the audit log. Settings and override changes are revisions with their audit row in one transaction, like every write |
| 7 | Samples are not configuration | pass | They are written by the sample service on its own connection with no revision; a rollback cannot restore or delete them; `tests/sampling_cluster.rs` checks that a sample write does not bump the version nor make the other instance reload |
| 8 | Memory is bounded | pass | The queue holds at most `queue_depth` samples (full means dropped and counted, never blocking the request); captured bodies draw on `body_budget_bytes`, released when the sample is written or dropped; `tests/sampling_under_load.rs` floods one route with connectivity failures while another serves traffic and checks the counters account for every candidate |
| 9 | The table is bounded | pass | Retention by age, rows and bytes, run by every instance; property test: after one pass the table is within all three limits, nothing older than the retention remains, and nothing needless is removed. `tests/sampling_cluster.rs` has two processes inserting and pruning one file with no write failing |
| 10 | An outage cannot fill the table or the disk | pass, after one fix (finding 1) | The storm guard keeps a burst and then one per interval per route, trigger and endpoint, and carries the suppressed count; the global rate limit caps the rest. `tests/sampling.rs` sends 1,000 failures: a handful are kept, the next one after the interval says it stands for hundreds; a slow request on another route during the outage is kept |
| 11 | Sampling cannot delay or fail a request | pass | The decision runs after the response; the hand-off is a non-blocking `try_send`; the writer is a separate thread and connection. `tests/sampling_under_load.rs`: every failure is a clean 502, the healthy route never fails, throughput is within a factor of two of the same load with sampling off (the benchmark in [benchmarks.md](benchmarks.md) measures the real difference), and a configuration write during the flood is not starved |
| 12 | Shutdown does not lose what was queued, and a full disk is an error not a crash | pass | `a_sample_still_waiting_for_its_batch_is_written_when_the_proxy_stops` and `shutdown_flushes_what_the_flood_queued`; the writer tests for a locked database and a full disk (spike G2 found the latter) |
| 13 | The database file is not readable by other users | pass, after one fix (finding 2) | A new database is created with mode 0600 on Unix, and SQLite gives the `-wal` and `-shm` files the same mode; a test checks all three, and that an existing file keeps its owner's mode |
| 14 | Metric labels carry no request value | pass | The only labels are `trigger` and `reason`, fixed sets; a unit test in the telemetry crate fails if another label appears |
| 15 | The sample API cannot be used to read the file system or run a query | pass | Filters are typed (a trigger name, a status class, numbers, bounded strings) and bound as SQL parameters; an invalid query is refused before it reaches the database |

## Findings

### 1. The token bucket refilled completely under concurrency (fixed)

The rate limiter and the storm guard share one lock-free bucket whose stored time is the low
32 bits of a millisecond clock. A caller reads the clock before it reaches the bucket, so with
several threads a caller can arrive holding a reading a few milliseconds **older** than the
one already stored. The wrapping subtraction turned that into a gap of about 49 days and
refilled the bucket to its cap. Under a real flood (eight keep-alive clients) the storm guard
kept hundreds of samples of one outage instead of a burst, and the global limit leaked the same
way. Found by the new load test (it reported 15 to 33 stored samples where the storm guard
allows 3 to 5). Fixed in `bucket.rs`: a reading less than a minute behind the stored time is a
late arrival that adds no credit and does not move the stored time back. Covered by unit tests
for the late reading, and by property tests that feed both limiters out-of-order clock readings
and assert they never admit more than the burst plus the rate; the unit test and the load test
both failed before the fix.

### 2. The database file was created with the default mode (fixed)

The file holds token hashes and certificate ciphertext (M3 and M5), and from M6 redacted
request data: client addresses, paths and, where a route opts in, bodies. It was created with
the process's umask, typically 0644. Fixed: a new database is created 0600 on Unix. An
existing file is left as its owner set it. On Windows the state directory's access rules apply.

### 3. The query string of `Referer` and `Location` was kept as sent (fixed)

Both headers are in the fixed set every sample keeps, and both hold a URL. A reset link or a
redirect with a token in its query would have been stored in clear although the same query
on the request itself is redacted. Found while checking the table above against the header
lists. Fixed: the query of a URL in either header goes through the same parameter redaction
as the request's. A unit test covers both headers and the cases that must stay unchanged, and
`sampling_hygiene.rs` sends a `Referer` with a secret; each fails without the fix.

## Known limits (what redaction does not do)

These are properties of the design, stated so that nobody relies on more than is there.

1. **A secret in a field that is on no list is stored** (a body field called `pin`, a header
   called `X-Session`, a query parameter called `s`). The lists are name-based. Operators add
   names under Settings; names containing `token`, `secret`, `password` or `passwd` are
   redacted whatever the list says (`key` and `auth` are not: too many harmless names). Bodies stay off by default for exactly this reason.
2. **A secret in the path is stored** (`/reset/ZQ123`, an API key as a path segment). The path
   is kept as sent, capped at 8 KiB. It is what the proxy routed on.
3. **A secret in free text** (a plain-text body that says "my password is hunter2") is not
   found. The text redactor blanks the value after a sensitive name, not prose.
4. **The access log is not redacted.** It is the operator's own file of raw request lines and
   has recorded the full request target, query string included, since M1. `sampling_hygiene.rs`
   documents this. Operators who put secrets in query strings should treat that file like the
   database. Redacting it is a change to an older contract and is left to the owner.
5. **Client addresses are personal data in some jurisdictions.** Every sample has one.
   Retention (7 days by default) and `DELETE /api/samples` are the controls; the table is
   not encrypted (the file is protected by its mode and the host).
6. **Backups.** A backup of the database now contains redacted request data. Protect it like the
   database; to leave samples out, copy the configuration through the revision API instead.
7. **The sample table is shared by every instance** with the database. An administrator on one
   instance reads the samples of all of them; that is the point of sharing, and the access rule
   is the same on each.

## Residual risk

A request that carried a secret in an unlisted place and was unusual enough to be sampled
leaves a copy of that secret in the database for the retention period, readable by an
administrator and by whoever can read the file. The mitigations are the defaults (bodies off,
7 days, 0600), the name-based built-ins, and the operator's lists. Nothing here makes that
zero, and the owner should decide whether the default retention and the body opt-in are the
right defaults for the first deployment.
