# ADR 0017: the audit log and the actor on every change

Status: proposed (awaiting owner confirmation); built in M5 PR3 (committed changes) and PR7a (the rest). Plan reference: M5 H7, H8.

## Context

Nothing records who changed what. The audit entry of a change must never drift
from the change itself, and tokens, audit entries and maintenance must not be
undone by a rollback.

## Decision

- **A table `audit_log`**, append-only from the application, pruned hourly by age
  (`retention_days = 90`) and count (`max_rows = 200000`), in batches, never
  removing the newest entries.
- **Two writers, no duplicates.**
  1. `ConfigService::apply` writes the audit row **in the same transaction** as the
     change and its revision, taking the actor from an explicit
     `ChangeContext { actor, role, source, request_id }` that every admin handler
     passes. The action name and target come from a small typed `Audited`
     description the mutation returns, so a handler cannot forget it.
  2. The admin middleware records what a transaction cannot: authentication
     failures, requests refused with 401, 403, 429 or 421, writes that failed
     validation (400, 409), and token-use events that matter.
- **An entry** holds time, instance id, actor (token name and role, `anonymous`, or
  `system` for the sweeper, `cli` for offline commands), source address, request
  id, a stable action name (`subsystem.update`, `maintenance.set`, ...), target
  names, outcome (`ok`, `denied`, `failed`), a short structured detail (at most
  2 KiB) and the revision it created. Never a body, secret, key or PEM.
- **`revision.actor`** names who made each configuration revision.
- **Tokens, audit settings and maintenance are not `Config`.** Like certificate
  material (ADR 0010) they never enter a revision's configuration copy, and a
  rollback never changes them. A change to any of them is a **summary-only
  revision** that bumps the cluster generation, so other instances reload it
  through the existing sync poll (ADR 0011).
- **API:** `GET /api/audit` (paged, filtered, newest first) and
  `GET /api/audit/export` (NDJSON, admin only).

## As built

- The middleware's entries (`auth.failed`, `access.denied`, `host.refused`,
  `request.rejected`, `token.first_use`) are written through
  `ConfigService::record_audit`, which never creates a revision, so a committed
  change is never recorded twice.
- A failed authentication carries its reason (`no token presented`, `unknown
  token`, `wrong secret`, `token revoked`, `token expired`) and whether it was the
  failure that tripped the throttle; the requests the throttle then refuses are
  not logged one by one.
- Entries for requests nobody authenticated (401 and a foreign `Host`) are capped
  at 60 a minute for the process; authenticated denials and rejections are not
  capped.
- The path is logged without its query: the WebSocket ticket lives there.

## Consequences

- One extra `INSERT` inside a transaction that already commits.
- `apply` changes signature for every caller; the `reload_under_load` loop guards
  that PR.
- Tamper evidence against someone with write access to the database is out of
  scope: that attacker owns the proxy anyway.
