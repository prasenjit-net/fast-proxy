# ADR 0022: samples live in the shared database

Status: accepted by the owner (2026-11-11); the write path (J17) stays to be confirmed by spike G2
(database contention). Plan reference: M6 J2, J10, J11, J17, J18.

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
