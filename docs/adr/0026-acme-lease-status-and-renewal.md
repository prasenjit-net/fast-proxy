# ADR 0026: the ACME lease, the status and the renewal policy

Status: proposed (2026-11-15); pending the owner's review. The policy and the tables are built
in PR1; the writes through `ConfigService` in PR2. Plan reference: M4 K6, K7, K10, K11, K14.

## Context

Only one instance may talk to the CA (ADR 0014). Rate limits punish loops of failed orders and
re-orders on restart. Operators need to see, per subsystem, what ACME is doing and why.

## Decision

- **Lease:** one row, `acme_lease (holder, expires_at)`, taken and renewed by compare-and-swap in
  one statement (`... ON CONFLICT DO UPDATE ... WHERE holder = me OR expires_at <= now`),
  renewed every `lease_ttl / 3`, released on shutdown. The runner places orders only while it
  holds the lease. A lost lease mid-order is safe: the next holder starts the order again.
- **Status:** one row per subsystem, `acme_status`: state (`idle`, `pending`, `valid`,
  `renewing`, `failed`, `rate_limited`, `inactive`), eligible and skipped names, the attached
  certificate, the next attempt, `failures` (since the last success, drives the backoff),
  `window_failures` and `failure_window_start` (drive the hourly cap), a sanitized single-line
  last error, the last attempt and success, and the instance that wrote it.
- **Lease and status are written without a revision**, through narrow `ConfigService` methods
  (`Database::write_unrecorded`), so `ConfigService` stays the only writer. A revision per lease
  renewal or status change would make every instance reload constantly. This is the second
  exception to "every write is a revision", after samples (ADR 0022); failures are still audited
  (`certificate.acme.fail`).
- **When to order** (`RenewalPolicy::decide`, pure, from stored facts only, so a restart never
  re-orders): nothing eligible is idle; a missing certificate is ordered at once; an added host
  is ordered 30 s after the last change (a bulk edit makes one order) while a removed host waits
  for the renewal; a certificate is due at two thirds of its lifetime less a stable
  per-subsystem jitter of up to 5% of the lifetime; a forced renewal skips the backoff but never
  the cap.
- **Backoff:** 1 min, 5 min, 30 min, 2 h, then 6 h, each varied by up to 10%; a permanent
  problem (`badDomain`, `rejectedIdentifier`, `unsupportedIdentifier`, `caa`, `dns`,
  `unauthorized`) waits 6 h; `rateLimited` waits until its `Retry-After` (1 h if none, never
  under a minute). **At most 5 failed orders per subsystem per window of one hour**, forced ones
  included: the fifth pushes the next attempt to the window's end and refuses a forced renewal
  (429).

## Defaults

| Rule | Default |
|---|---|
| Renewal point | 2/3 of the lifetime, less up to 5% jitter |
| Backoff ladder | 1 min, 5 min, 30 min, 2 h, 6 h (±10%) |
| Permanent problem | 6 h |
| Rate limited without `Retry-After` | 1 h |
| Failure cap | 5 per subsystem per hour |
| Host change debounce | 30 s |
| `lease_ttl` | 10 min (renewed every third) |
| `renew_check_interval` | 1 h |

## Consequences

- Every instance shows the same status; only the lease holder changes it.
- The policy is property-tested: due times stay in their jitter band and before expiry; no
  attempt is sooner than the shortest backoff; no window holds more failures than the cap.
