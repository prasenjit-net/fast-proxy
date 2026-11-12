# ADR 0022: samples live in the shared database

Status: accepted by the owner (2026-11-11); J17 confirmed by spike G2 (see the result below). Plan
reference: M6 J2, J10, J11, J17, J18.

## Context

Several instances can share one database (M3). Samples must be readable from any instance,
survive a restart, and stay bounded, without making the request path wait and without making
instances reload configuration on every sample.

## Decision

- **All sample data is stored in the database**, in a `sample` table (short indexed columns
  for filters, a JSON `detail` column, no foreign keys). There is no ring buffer and no spill
  file. A sample is lost only if the process dies before its batch is written (about 250 ms).
- **Samples are cluster-wide.** Every instance writes its own rows (each stamped with its
  `instance`), any instance's admin API reads all of them, and ids come from the database.
  What stays local to an instance because it describes its own traffic: latency windows, the
  client table, the storm guard, the rate limiter and the body budget.
- **The write path is an exception to "all writes go through `ConfigService::apply`"**, which
  is about configuration. A revision, an audit row and a snapshot swap per sample would make
  every instance reload on every sample. Samples are written by a **sample service**
  (`fast-proxy-sample-service`) straight through the store: **no revision, no audit row per
  sample, no change of the cluster generation**. Changes to the sampling *settings* still go
  through `ConfigService::apply`; `clear` and `export` are single audited events.
- **Hand-off.** Storm guard, then a token bucket (20 per second per instance, burst 40), then
  a bounded channel (`try_send`; full drops and counts `queue_full`), then the sampler task
  redacts and batches (50 samples or 250 ms) and writes one transaction per batch. A failed
  write drops the batch, counts `store_error` and backs off; it never retries without bound.
- **A second connection and writer thread** for samples, so a burst of sample writes does not
  queue in front of a configuration write on the store thread. Batches are small and short and
  use a busy timeout. If spike G2 shows configuration writes slowing, the fallback is a
  separate database file for samples (same code, a second path).
- **Crates.** `fast-proxy-sampling` is pure and defines a `SampleSink` trait.
  `fast-proxy-sample-service` (sampling, store, model) holds the writer, the pruner and the
  queries. The data plane depends on `sampling` only and never touches the store. The `store`
  crate sees opaque columns and has no cryptography dependency.
- **Retention is shared and live**, in the settings row: `retention_days`, `max_rows`,
  `max_bytes`, whichever is hit first. **Every instance** runs the pruner every 30 seconds,
  deleting the oldest 500 rows per statement and yielding between statements; it is idempotent
  and needs no leader. Totals come from one indexed aggregate per pass.
- **Portable SQL** for the later PostgreSQL project: millisecond integers, a trigger bit set
  instead of a JSON filter, database-assigned ids.

## Defaults

| Setting | Default |
|---|---|
| `retention_days` | 7 |
| `max_rows` | 100 000 |
| `max_bytes` | 256 MiB |
| Rate limit | 20 per second per instance, burst 40 |
| Queue depth | 1024 |
| Batch | 50 samples or 250 ms |
| Pruner | every 30 s, 500 rows per statement |

## Consequences

- An operator sees one list whichever instance they reach.
- The database file now holds redacted request data: backups and copies carry it, which the
  docs and the security review state. Body capture is off by default and retention is short.
- The cluster total is bounded by retention, not by the per-instance rate limit.

## Spike G2 result (2026-11-12): database contention and the hand-off

The same throwaway benchmark, SQLite in WAL mode with `synchronous = NORMAL` and a 5 s busy
timeout, a table of 100 000 samples (about 1.5 KiB of `detail` each, 212 MB), and a configuration
write of one 20 KB revision plus an update every 4 ms. Single laptop, shared machine: the idle
run's own p99 moved between 1.4 and 5 ms from run to run, so read the p99 column as a bound.

| Scenario (configuration-write latency) | p50 | p99 | max |
|---|---|---|---|
| Idle | 193 us | 5.0 ms | 34 ms |
| Sample writer, 200 rows per second (10 times the default), own connection | 344 us | 9.6 ms | 16 ms |
| Sample writer, 2000 rows per second (100 times the default), own connection | 266 us | 13.9 ms | 22 ms |
| 2000 rows per second plus the pruner (500-row deletes, held the table at 100 000) | 300 us | 7.9 ms | 33 ms |
| 2000 rows per second, writer in **another process** | 322 us | 10.5 ms | 12.7 ms |
| 200 rows per second, writer in another process | 277 us | 12.3 ms | 24 ms |

- A 50-row batch holds the write lock for about 0.3 ms (p50) and at most about 8 ms (p99). That is
  also the longest a configuration write waits for a batch, so **no separate database file is
  needed** (J17 confirmed): the median configuration write is unchanged at 100 times the default
  rate, no write came near the busy timeout, in-process and across processes, and the pruner kept
  the table bounded while the writer ran without starving it.
- The own connection does not remove the lock wait (SQLite has one writer); it keeps sample
  batches out of the store thread's queue and out of its transactions. The hand-off is the
  important bound, below.
- **`try_send` costs about 130 ns** when it fails (a full queue: the sample is dropped) and about
  0.9 us when it succeeds with the receiver waking up, only for kept samples (at most the rate
  limit, 20 per second by default). A request that is not sampled pays nothing.
- **The list query** (a subsystem and 5xx, 50 rows, newest first) took about 4 ms at p50 on 100 000
  rows. Its p99 was unstable (5 ms in one run, 480 ms in another right after the table was filled,
  most likely a cold cache and a checkpoint), so it is **not yet a proven bound**: task SV3 and Q3
  re-measure it, and a covering index for the common filters is the first remedy.
- **An unthrottled writer is dangerous.** The first run, with an unlimited writer and no pruner,
  filled the disk within seconds (a 1.4 GB database and a 3 GB WAL, the checkpoint could not keep
  up) and the spike then failed with `database or disk is full`. The design already prevents this
  (the writer only drains a bounded queue fed by a rate-limited path, and the pruner bounds the
  table), but it adds two requirements: the writer must treat a full disk as an ordinary
  `store_error` (drop the batch, count it, back off; test it in SV1) and the connection sets a
  `journal_size_limit` so the WAL is truncated after a checkpoint.
- Not measured: Linux and Windows, a spinning disk, `synchronous = FULL`, a network file system
  (SQLite on one is not supported anyway). Q3 and Q6 repeat the key runs against the real stack.
