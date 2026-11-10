# M5 Plan: Operations control (maintenance mode, admin authentication, audit log, CLI)

Status: **draft v0.1 (2026-11-10), proposed; PR1 (ADRs 0015 to 0019, as proposed) committed, awaiting your confirmation of the (confirm) items; PR2 (model, store, auth crate) written, awaiting review.** M4 (ACME) is deliberately skipped for now
and will be planned later; section 11 lists what M5 must leave room for so M4 can land
afterwards without rework.

Items marked **(confirm)** need the owner's sign-off before the tasks that depend on
them start. Parent document: [ROADMAP.md](ROADMAP.md) (section 4.5 and the M5 list).
Predecessor: [M3-PLAN.md](M3-PLAN.md) (complete, its stack of pull requests #29 to #41
still to be merged; section 11 of that plan lists what carries over). Task sizes are
relative effort (S under half a day, M about 1-2 days, L about 3-5 days), not
commitments. Tick the checkboxes as tasks land.

## 1. Goal and exit criteria

Until now the admin plane trusts whoever can reach it (it binds to loopback and warns
otherwise), nothing records who changed what, and the only way to stop traffic to a
backend is to disable a subsystem, which answers a bare 503 with no explanation, no
expiry and no way for an operator to test the fix before reopening. M5 makes the proxy
**safe to operate**: the admin plane needs a credential, every change is attributed and
auditable, and an operator can put a whole proxy, a subsystem or one route into a
**temporary, reversible maintenance mode** with a proper response, a TTL and a bypass
for the people verifying a deploy. A small **CLI** does the common operations from a
shell.

**Exit criteria** (from the roadmap, made measurable):

1. **The admin plane requires authentication.** Every API route and the WebSocket need
   a valid token; a missing or wrong one is 401, a read-only token trying a write is
   403; the health endpoint alone is open. The first start creates a bootstrap admin
   token (shown once), so a fresh install is never open and never locked out, and
   losing every token is recoverable from the machine with the CLI.
2. **Tokens are managed live** in the UI and API (create with a name and a role, list,
   revoke), stored only as hashes (the database holds no usable credential), shown
   once at creation, never logged, and effective on every instance sharing the
   database within the sync interval.
3. **The admin plane is protected in transit and against browsers.** The admin
   listener can serve HTTPS; binding it to a non-loopback address without HTTPS and
   authentication is refused at startup; a `Host` header that is not an allowed admin
   name is rejected (DNS rebinding); responses carry the security headers; repeated
   authentication failures from one source are slowed down.
4. **Every change is audited.** A write records who (token name and role), when, from
   where, what and with what result, atomically with the change; denied and failed
   attempts and authentication events are recorded too; the history is browsable,
   filterable and exportable, bounded by a retention setting, and never contains a
   secret, a key or PEM. Each configuration revision names its actor.
5. **Maintenance mode** works at three scopes: the whole proxy, a subsystem, or one
   route. A request in a maintenance scope gets a **503 with `Retry-After`** and a
   configurable page (text, HTML or JSON) and **never reaches an upstream**. A toggle
   can carry a **TTL** and then ends by itself, even if nobody is looking.
6. **Bypass rules** let chosen source networks, or requests with a secret header,
   through a maintenance scope so a deploy can be verified before reopening.
7. **Maintenance is persistent and live**: a restart during maintenance stays in
   maintenance, a toggle takes effect for the next request without dropping a
   connection, another instance sharing the database follows within the sync
   interval, and health checks keep running so the group is ready on exit.
8. **The CLI** can do the common operations against a running proxy (`maintenance
   on|off|status`, `reload`, `status`), validate a configuration without starting
   (`config validate`), and manage tokens offline (`token create|list|revoke`).
9. **It is all observable**: authentication results, audit volume, maintenance state
   and the requests it answered are metrics; the dashboard shows an active maintenance
   banner; the UI has pages for sign-in, tokens, the audit log and maintenance.
10. **No regression, and no cost when idle:** the M1 to M3 exit criteria still pass, and
    with no maintenance active the data plane's throughput stays within the M3
    baseline (section 8).

## 2. Scope

### In scope

- Authentication of the admin plane: bearer tokens with two roles, a bootstrap and a
  recovery path, a short-lived ticket for the WebSocket, a failure throttle, a `Host`
  allow-list and security headers.
- TLS on the admin listener, and the refusal to expose an unprotected admin plane.
- The audit log: storage, the actor on every change and revision, denied attempts and
  authentication events, retention, API, export and UI.
- Maintenance mode: model, storage, snapshot, data plane behaviour, bypass, TTL and its
  expiry, API, events, metrics and UI.
- CLI subcommands, built on the admin API, plus offline token management.
- Admin UI pages: sign-in, tokens, audit log, maintenance (overview and the controls on
  subsystem and route pages), a cross-subsystem routes and upstreams overview, and the
  roles reflected in what the UI offers.
- The M3 carry-over items in workstream C.

### Out of scope (later milestones, or never)

| Item | Where |
|---|---|
| ACME: the client, HTTP-01, renewal, per-subsystem ACME (design constraints in section 11) | M4 |
| Request sampling | M6 |
| Rate and connection limits for data-plane clients, general rate limiting, 1.0 hardening | M7 |
| Per-user accounts, passwords, MFA, SSO (OIDC or SAML), LDAP | not planned: tokens are enough for a single-binary ingress; revisit after 1.0 |
| Roles beyond `admin` and `viewer`, per-subsystem permissions | revisit after M5 **(confirm, H1)** |
| Shipping the audit log to an external system (syslog, a SIEM) | after 1.0; the NDJSON export and the console `tracing` target cover it |
| Maintenance schedules (cron windows), per-client maintenance, canary or partial maintenance | not planned; a TTL and the CLI cover the real need |
| A network database and running many instances against it | the later PostgreSQL project (M5 keeps the M3 cluster design: everything shared lives in the database) |
| Signed or encrypted audit entries, tamper evidence against someone with database write access | not planned (an attacker with write access to the database owns the proxy anyway) |
| Changing the data plane's own listener authentication (it is the public side) | never |

## 3. Design decisions for M5

Items marked **(confirm)** are recommendations that need your sign-off.

| # | Decision | Recommendation |
|---|---|---|
| H1 | Authentication model | **Bearer tokens**, `Authorization: Bearer fp_<id>.<secret>`. A token has a unique name, a role and optional expiry. Two roles: **`admin`** (everything) and **`viewer`** (every read, no write, no key material, no token or audit-export management). No passwords, no cookies, no sessions: nothing for CSRF to ride on, nothing to store server-side but a hash. The secret is 256 random bits, shown **once** at creation; the database keeps `id` and `SHA-256(secret)` only, compared in constant time (a fast hash is right for a high-entropy random secret; there is nothing to brute-force). A token can be revoked at once; the last admin token cannot be revoked or demoted (409), so the API cannot lock itself out. **(confirm)** |
| H2 | Bootstrap and recovery | With no admin token in the database, the first start **generates** one named `bootstrap`, prints it once to the console and the log marker `admin token created` (never again), and stores its hash. Containers can instead pass `FAST_PROXY_BOOTSTRAP_TOKEN` (used only while no admin token exists). **Recovery from the machine**: `fast-proxy token create --role admin` works directly on the database file (offline or while running) for anyone who can read the state directory, who could anyway. **(confirm)** |
| H3 | Authentication is on by default | `[admin] auth = "required"` is the default. `auth = "off"` is accepted **only while the admin listener is bound to loopback**, logs a warning on every start, and is what the test harness sets. There is no "open when no token exists" mode: the bootstrap token makes a secure default as easy as an insecure one. **(confirm)** |
| H4 | Admin TLS and exposure | `[admin.tls] enabled`. The admin listener uses a certificate **stored in the database like every other** (M3: no certificate files): by default a self-signed `admin` certificate generated for the configured names the first time it is needed, replaceable on the Certificates page (an "admin certificate" card) with an uploaded one, by name. **Binding the admin listener to a non-loopback address requires both TLS and authentication**, otherwise startup fails with a message naming the key to set; `[admin] allow_insecure_remote = true` is the explicit, warned override. The M3 loopback rule for requests that carry a private key (F27) becomes: allowed for an `admin` token **over a protected channel** (loopback, or admin TLS on), refused otherwise. **(confirm)** |
| H5 | The WebSocket | Browsers cannot set headers on a WebSocket. `POST /api/ws-ticket` (authenticated) returns a **single-use ticket valid for 30 seconds**; `/ws?ticket=...` consumes it and the socket carries the ticket's role. Tickets are in memory on the instance that issued them. The admin access log **redacts the query string of `/ws`**. A viewer's socket receives the same events (they hold no secrets: names and counts). **(confirm)** |
| H6 | Browser and brute-force protection | A **`Host` allow-list** (`[admin] allowed_hosts`, default: the listen address, `localhost`, `127.0.0.1`, `[::1]`) rejects other `Host` values with 421, which stops DNS rebinding against a loopback admin plane. Responses carry `Content-Security-Policy` (self only), `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `Cache-Control: no-store` on the API. **Failure throttle**: after 10 failed authentications from one source address within a minute that address gets 429 with `Retry-After` for a minute (instance-local counters; a valid token from a throttled address is still refused until the window ends, so a flood cannot be used to guess faster). Real rate limiting is M7. **(confirm)** |
| H7 | Audit log design | A table `audit_log` in the database, append-only from the application, with retention (`[admin.audit] retention_days = 90`, `max_rows = 200000`, pruned hourly). Two writers, no duplicates: (1) **`ConfigService::apply`** writes the audit row **in the same transaction** as the change and its revision (so a committed change always has its audit entry and vice versa), taking the actor from an explicit `ChangeContext { actor, source, request_id }` passed by every admin handler; (2) the **admin middleware** records what a transaction cannot: authentication failures and successes of token use that matter (a token's first use, failures), requests refused with 401, 403, 429 or 421, writes that failed validation (400, 409), and token management. Entries carry: time, instance id, actor (token name, role), source IP, request id, action (a stable name such as `subsystem.update`), target, outcome, a short structured detail (names, ids, the revision number), never a body, a secret, a key or PEM. Each `revision` row gets an `actor` column. **(confirm)** |
| H8 | Tokens, audit and maintenance are not `Config` | Like certificate material (M3 F26): they never enter a revision's configuration copy and **a rollback never changes them** (rolling back must not revoke today's tokens, erase the audit log or silently switch maintenance on or off). A change to any of them is a **summary-only revision** that bumps the cluster generation, so other instances reload them through the existing sync poll. **(confirm)** |
| H9 | Maintenance model | Three scopes: **global** (one row), **subsystem**, **route**. Each active scope has: a message, an optional response body override, a `Retry-After` (seconds; default derived from the TTL, else 120), optional **bypass rules**, an optional **expiry** and who set it and when. State lives in a `maintenance` table (one row per target, cascade on delete of the subsystem or route). Precedence is "any applicable active scope answers": global first, then subsystem, then route. A request must be allowed by **every** active scope that applies to it (bypass rules are per scope, so bypassing a route does not bypass a subsystem-wide maintenance). **(confirm)** |
| H10 | The TTL | Stored as an **absolute** `expires_at` (wall clock, milliseconds). The data plane checks it **at request time** (one clock read, only when a maintenance scope applies), so expiry is exact and needs no write; a **sweeper** in the config service then clears expired rows with a normal write (revision, audit entry `maintenance.expired`) so the database, the UI and other instances agree. Every instance runs the sweeper; clearing an already-cleared row is a no-op. Clock skew between instances is bounded by NTP and documented. A restart keeps the expiry. **(confirm)** |
| H11 | The response | **503 Service Unavailable**, `Retry-After`, `Cache-Control: no-store`, and a body chosen by the client's `Accept`: JSON (`{"error":{"code":"maintenance","message":...,"retryAfterSecs":...}}`), or HTML or plain text. The default body is a built-in page showing the message; a scope may instead supply its own body (up to 16 KiB) and content type (`text/html`, `text/plain` or `application/json` only). The message is HTML-escaped in the built-in page (it is operator text, but the page is served to the public). The `X-Request-Id` header is set as for every fixed response. **(confirm)** |
| H12 | Bypass rules | Per scope: up to 32 **source CIDRs** and/or one **secret header** (`name` plus a secret). The source address is the **effective client address** used elsewhere (the peer address, or `X-Forwarded-For` only when the peer is in `trusted_proxies`), so a client cannot talk its way past with a forged header. The header secret is stored as a SHA-256 hash like a token (shown once, generated by the server if not given); the data plane hashes the presented value and compares in constant time, only when the header is present. A bypassed request is served normally and gets `X-Maintenance-Bypass: <scope>` so the operator can see it worked; bypass use is counted, not logged line by line. **(confirm)** |
| H13 | Where it sits and what it never touches | After the host matched a subsystem and the 421 and redirect checks, **before routing**: global, then subsystem; then after the route matched, the route scope; always before an endpoint is selected, so a maintenance request consumes no retry budget and touches no endpoint state. It applies to **upgrade requests** too (a WebSocket handshake gets the 503); connections that are **already upgraded stay open** (draining them is the operator's choice: disable the subsystem). **Health checks keep running** (they do not go through the data plane), and the **admin plane is never in maintenance**. The ACME HTTP-01 challenge path (M4) must bypass maintenance exactly like the redirect, so the check is one function (section 11). |
| H14 | Metrics and the error rate | `fastproxy_maintenance_active{scope,subsystem,route}` (1 while active), `fastproxy_maintenance_responses_total{scope,subsystem,route}` and `fastproxy_maintenance_bypass_total{scope,subsystem}`, labels from configuration names only. Maintenance 503s are **not counted in the dashboard's 5xx error rate** (they are intended): they are recorded in `requests_total` with `status_class="5xx"` like any 503 but flagged in the access log (`maintenance=<scope>`), and the traffic sampler subtracts the maintenance counter from the error rate. **(confirm)** |
| H15 | CLI shape | `fast-proxy` with no subcommand runs the server exactly as today. New subcommands: **`maintenance status|on|off`** (scope flags, `--ttl`, `--message`, `--retry-after`, `--bypass-cidr`, `--bypass-header`), **`reload`**, **`status`** and **`audit`** (tail) talk to a running proxy through the admin API (`--url`, a token from `--token`, `FAST_PROXY_TOKEN` or `--token-file`, `--ca-file` or `--insecure` for admin TLS); **`config validate`** parses and validates the configuration file and, with `--database`, compiles the stored configuration, without starting anything; **`token create|list|revoke`** work offline on the database file. Exit codes: 0 success, 1 the proxy refused or the validation failed, 2 usage or connection errors. The client reuses the HTTP stack already in the tree (hyper and hyper-rustls): no new HTTP dependency. **(confirm)** |
| H16 | What "reload" means | The configuration is live, there is no file to reload for subsystems. `reload` (CLI) and `POST /api/reload` ask the instance to **re-read the database now** and recompile (the sync poll's reload without waiting for the interval or for a newer version), for the operator who edited the database directly, runs with `config_poll_interval = "0s"`, or wants to be sure after a restore. It returns the version now live. Process settings (`config.toml`) still need a restart; `config validate` checks the next one. **(confirm)** |
| H17 | Crates | New crate **`fast-proxy-auth`** (token format, generation, hashing, constant-time verification, roles and permissions, the failure throttle, the ticket store): no HTTP and no database, depends on `model` only. `model` gains the maintenance types and the `Role`/`Action` vocabulary; `store` the tables as opaque rows (it never hashes or verifies); `routing` carries compiled maintenance in the snapshot; `config-service` owns the writes, the actor context, the audit transaction, the sweeper and the token cache; `dataplane` does the check and the response; `admin` does middleware and endpoints; the binary the CLI. Dependencies still point one way. **(confirm)** |
| H18 | The UI's token | The UI asks for a token on first use, keeps it in `sessionStorage` (gone when the tab closes), sends it as a header, and on any 401 returns to the sign-in page. The admin CSP forbids inline script and third-party origins, which is what makes `sessionStorage` an acceptable place for a bearer token; the sign-in page says so and offers a sign-out that clears it. A `viewer` token shows the UI read-only (write controls hidden, not just refused). **(confirm)** |
| H19 | Compatibility | None kept: the project is undeployed. The schema is edited in place (version 3 to 4). Existing tests and the UI development workflow move to authentication through one switch (`auth = "off"` on loopback in the test harness and a token header in the HTTP-level tests), see workstream Q. |

## 4. Target architecture

```
                 config.toml                                  SQLite (shared by instances)
 [admin] auth, allowed_hosts, tls, audit retention     admin_token  (id, name, role, secret_hash, expiry, revoked)
 [admin.tls] names                                     audit_log    (append-only, retention)
 (the data plane settings are unchanged)               maintenance  (global | subsystem | route, TTL, bypass, body)
                                                       revision     (+ actor)      certificate (the admin certificate)
        |                                                     |
        v                                                     v
  fast-proxy-auth                               ConfigService (the one writer)
  token format, SHA-256 hash, constant-time     apply(ChangeContext, mutation): lock -> transaction -> mutate
  roles/permissions, throttle, tickets          -> audit row in the same transaction -> veto by compiling
                                                -> commit with revision (+actor) -> swap -> broadcast
        |                                       sweeper: expired maintenance -> cleared by a normal write
        v                                       token cache: reloaded when the cluster generation moves
  admin plane (Axum)
   Host allow-list -> security headers -> authenticate (token | ws ticket) -> authorize (role x action)
   -> throttle -> handler (ChangeContext from the token) -> audit of denials and failures
   /api/tokens  /api/audit  /api/maintenance...  /api/ws-ticket  /api/reload  admin TLS listener
        |
        v
  data plane request path (unchanged order, one new step):
   host -> subsystem -> [421 / redirect] -> MAINTENANCE: global, subsystem -> route -> MAINTENANCE: route
        -> select endpoint -> forward
   maintenance: any applicable scope active and not expired? -> bypass (CIDR / secret header)? -> else 503
```

Crate dependencies (one way, no cycles):

```
model <- auth                         (no I/O, no HTTP)
model <- tls <- routing <- dataplane
model <- store                        (tokens, audit and maintenance rows are opaque or plain data here)
auth + tls + routing + store + balancing + telemetry <- config-service
config-service + auth + tls + health <- admin
everything <- the binary (which also holds the CLI)
```

### 4.1 One authenticated admin request, in order

1. The admin listener accepts (over TLS when `[admin.tls]` is on). A `Host` that is not
   allowed is answered 421 before anything else.
2. The throttle checks the source address; a throttled address gets 429.
3. The `Authorization: Bearer` token is parsed (`fp_<id>.<secret>`), the id looked up in
   the in-memory token cache, `SHA-256(secret)` compared in constant time, expiry and
   revocation checked. The `/api/health` route and the static UI assets skip this; the UI
   assets contain no secret and the page asks for a token.
4. The role is checked against the action the route needs (every route declares one).
   A `viewer` on a write is 403; an `admin` request carrying key material also needs a
   protected channel (H4).
5. The handler builds a `ChangeContext` from the token and the request id and calls
   `ConfigService::apply`, which writes the change, its revision and its audit entry in
   one transaction. Refusals and failures in steps 1 to 4, and validation failures, are
   recorded by the middleware.
6. Responses carry the security headers and the request id; the admin access-log line
   names the actor (never the token).

### 4.2 One request in maintenance, in order

1. The data plane loads the snapshot once (as always) and matches host and subsystem.
2. The 421 rules and the redirect run first (an unknown host is still a 404, a plain
   request still redirects to HTTPS: maintenance is served on the same terms as the site).
3. The global scope, then the subsystem scope, are looked up: a field read of an
   `Option` in the snapshot. When one is active (not past `expires_at`) the request is
   checked against that scope's bypass rules; if not bypassed, the 503 is built and sent.
4. The route is matched; the route scope is checked the same way.
5. Only then is an endpoint selected. A maintenance response touches no endpoint, no
   retry budget and no health state, and is counted in the maintenance metrics.

### 4.3 Maintenance with several instances

```
instance A: operator turns on maintenance for "shop" with a 30 minute TTL
            -> write (revision N+1, summary-only, audit maintenance.set, expires_at = now + 30m) -> swap
instance B: sync poll (5 s): version N+1 > mine -> reload -> maintenance is in B's snapshot
both:       every request checks expires_at against its own clock
at +30m:    requests are served again on both at once (the check is the clock);
            each sweeper writes or no-ops the clearing; the audit log has one maintenance.expired
```

## 5. Configuration and data model changes

### 5.1 Config file (`config.toml`) and command line

```toml
[admin]
listen = "127.0.0.1:9000"
auth = "required"              # "required" (default) | "off" (loopback only, warned)
allowed_hosts = []             # extra names accepted in the Host header (listen address and localhost are implicit)
allow_insecure_remote = false  # the explicit, warned override of H4

[admin.tls]
enabled = false                # required when listen is not a loopback address
names = ["localhost"]          # names (and IPs) of the generated self-signed admin certificate
certificate = "admin"          # the stored certificate to serve, by name (generated if absent)

[admin.audit]
retention_days = 90
max_rows = 200000
```

`FAST_PROXY_BOOTSTRAP_TOKEN` (environment) seeds the first admin token. Command-line
flags: `--admin-auth required|off`, `--admin-tls`, and the subcommands of H15. Startup
rules: a non-loopback `admin.listen` needs `admin.tls.enabled` and `auth = "required"`
unless `allow_insecure_remote`; `auth = "off"` on a non-loopback address is refused even
then; unknown keys are rejected as before; no secret is ever read from this file.

### 5.2 Database (schema 3 to 4, edited in place; no migration)

```sql
CREATE TABLE admin_token (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  role        TEXT NOT NULL,                 -- admin | viewer
  secret_hash BLOB NOT NULL,                 -- SHA-256 of the secret; the secret is never stored
  created_at  INTEGER NOT NULL,
  created_by  TEXT NOT NULL,                 -- the creating token's name, or 'bootstrap' / 'cli'
  expires_at  INTEGER,                       -- ms since the epoch, NULL = never
  revoked_at  INTEGER,
  last_used_at INTEGER                       -- coarse (about a minute), for the UI
);

CREATE TABLE audit_log (
  id          INTEGER PRIMARY KEY,
  at          INTEGER NOT NULL,              -- ms since the epoch
  instance    TEXT NOT NULL,
  actor       TEXT NOT NULL,                 -- token name, 'anonymous' (failed auth) or 'system' (the sweeper)
  role        TEXT NOT NULL,
  source      TEXT NOT NULL,                 -- client address
  request_id  TEXT NOT NULL,
  action      TEXT NOT NULL,                 -- stable names: subsystem.update, certificate.upload, maintenance.set, ...
  target      TEXT NOT NULL,                 -- names, never bodies
  outcome     TEXT NOT NULL,                 -- ok | denied | failed
  detail      TEXT NOT NULL,                 -- short JSON: ids, the revision number, the error code; no secrets
  revision    INTEGER                        -- the revision this change created, if any
);
CREATE INDEX audit_log_at ON audit_log (at);

ALTER revision ADD actor TEXT NOT NULL DEFAULT 'system';   -- shown as additions; edit CREATE TABLE directly

CREATE TABLE maintenance (
  id             INTEGER PRIMARY KEY,
  scope          TEXT NOT NULL,              -- global | subsystem | route
  subsystem_id   INTEGER REFERENCES subsystem(id) ON DELETE CASCADE,
  route_id       INTEGER REFERENCES route(id) ON DELETE CASCADE,
  message        TEXT NOT NULL DEFAULT '',
  body           TEXT NOT NULL DEFAULT '',   -- optional override, at most 16 KiB
  content_type   TEXT NOT NULL DEFAULT '',   -- text/html | text/plain | application/json when body is set
  retry_after_secs INTEGER,                  -- NULL = derive from the TTL, else 120
  bypass_cidrs   TEXT NOT NULL DEFAULT '[]', -- JSON array, at most 32
  bypass_header  TEXT NOT NULL DEFAULT '',   -- header name, or ''
  bypass_secret_hash BLOB,                   -- SHA-256 of the secret
  expires_at     INTEGER,                    -- ms since the epoch, NULL = until turned off
  set_by         TEXT NOT NULL,
  set_at         INTEGER NOT NULL
);
CREATE UNIQUE INDEX maintenance_target
  ON maintenance (scope, COALESCE(subsystem_id, 0), COALESCE(route_id, 0));
```

Rules (code, and tests): `scope` is one of three and the ids match it (global has none,
subsystem has `subsystem_id`, route has `route_id`); a role is one of two; the last
non-revoked, non-expired `admin` token cannot be revoked, expired or demoted; a body
needs a content type from the allowed three; CIDRs parse and number at most 32; the
header name is a valid header name and a secret needs a name; `retry_after_secs` is 1 to
86,400; the audit `detail` is at most 2 KiB. Token, audit and maintenance rows are
**not** part of `Config` and never enter a revision's configuration copy (H8). The SQL
stays portable (BLOB to BYTEA, millisecond BIGINT times, JSON as text) like the
certificate SQL (M3 F9).

### 5.3 API shape (sketch)

```
POST   /api/tokens                    { name, role, expiresInDays? }  -> { token: "fp_...", id, ... }  the secret appears ONLY here
GET    /api/tokens                    list: id, name, role, createdAt/By, expiresAt, revokedAt, lastUsedAt (no secret, no hash)
DELETE /api/tokens/{id}               revoke (409 for the last admin token)
GET    /api/whoami                    { name, role, tokenExpiresAt, protectedChannel }
POST   /api/ws-ticket                 -> { ticket, expiresInSecs }
GET    /api/audit?limit&before&actor&action&outcome&since   newest first, paged
GET    /api/audit/export?since=       NDJSON stream (admin only)
GET    /api/maintenance               every active scope: scope, target names, message, expiresAt, secondsLeft, setBy, bypass summary (never a secret)
PUT    /api/maintenance/global        { message?, body?, contentType?, retryAfterSecs?, ttlSecs?, bypass: { cidrs, header?, secret? } }  -> turns it on (or updates)
DELETE /api/maintenance/global        turns it off
PUT    /api/subsystems/{id}/maintenance            the same for a subsystem
DELETE /api/subsystems/{id}/maintenance
PUT    /api/subsystems/{id}/routes/{route_id}/maintenance
DELETE /api/subsystems/{id}/routes/{route_id}/maintenance
POST   /api/reload                    re-read the database now (H16): { version }
```

A bypass secret is accepted on write and returned **once** (when generated by the
server); reads show only that one is set. The WebSocket gains `maintenanceChanged`
(scope, target, active, expiresAt) and a `tokenChanged` event (names only). Every route
declares the action it needs (`read`, `write`, `keys`, `tokens`, `audit-export`), and the
role table is one function with an exhaustive test.

## 6. Tasks

Task IDs are referenced by the dependency table in section 7. Workstream letters:
**G** groundwork, **MD** model, **S** store, **AU** the auth crate, **CS** config
service, **R** routing and compile, **D** data plane, **A** admin API and security,
**AT** admin TLS, **CL** CLI, **U** admin UI, **Q** quality, **W** documentation, **C**
carry-over.

### Workstream G: Groundwork

- [ ] **G1 (S) Decisions and ADRs.** Get the **(confirm)** items in section 3 resolved.
  Write `docs/adr/`: 0015 admin authentication, roles, bootstrap and recovery
  (H1-H3, H5, H6, H18), 0016 admin TLS and exposure (H4), 0017 the audit log and the
  actor on every change (H7, H8), 0018 maintenance mode: model, TTL, response, bypass,
  placement (H9-H14), 0019 the CLI and reload (H15, H16).
  *Done when:* the ADRs are merged and no **(confirm)** item is open.
- [x] **G2 (S) Spike: the admin listener over TLS and the WebSocket ticket.** A
  throwaway test that (a) serves the Axum admin router through the existing TLS stack
  (`tokio-rustls`, the `tls` crate's server config, a fixed-certificate resolver) with
  `ConnectInfo` still working for the peer address, (b) upgrades a WebSocket through it,
  and (c) shows a `Host` check and a constant-time hash comparison working under the
  same middleware stack. Record findings in ADR 0016.
  *Done when:* the findings are in ADR 0016 and H4 and H5 are confirmed or changed.
- [x] **G3 (S) Defaults table.** Turn H1-H14 into the documented defaults (section 5
  and the config docs): the single source for validation and the UI.
  *Landed in PR1:* the `Defaults` tables of ADRs 0015, 0016 and 0018 plus section 5.1.

### Workstream MD: Model

- [x] **MD1 (M) Maintenance types.** `MaintenanceScope`, `MaintenanceSettings`
  (message, optional body and content type, retry-after, bypass CIDRs and header, expiry)
  and their validation with field-specific messages (`bypass.cidrs`, `retryAfterSecs`,
  `body`); the pure `is_active(now)` and `retry_after(now)` rules.
  *Done when:* table-driven tests accept and reject every bound and the expiry and
  derived `Retry-After` rules are covered including the exact expiry millisecond.
- [x] **MD2 (S) Roles and actions.** `Role` (admin, viewer), `Action` (read, write, keys,
  tokens, audit-export), the permission table as one exhaustive function, and the
  `TokenName` rule (1 to 63 characters, unique).
  *Done when:* a test enumerates every role and action pair and asserts the table.

### Workstream S: Store

- [x] **S1 (S) Schema.** Edit `schema.sql` in place with 5.2; bump `SCHEMA_VERSION` to 4.
  *Done when:* a fresh database opens and a version-3 file is rejected with the existing
  clear error.
- [x] **S2 (M) Token repository.** Create, list, find by id, revoke, mark used (coarse),
  count of usable admin tokens; hashes are opaque bytes and never returned by list
  functions meant for the API.
  *Done when:* tests cover uniqueness (409), revocation, the last-admin protection and
  that no listing function exposes `secret_hash`.
- [x] **S3 (M) Audit repository.** Append inside a caller's transaction, page by time and
  id, filter by actor, action, outcome and time, prune by age and count, export in
  order; the `detail` size limit.
  *Done when:* tests cover paging stability, each filter, pruning at both limits, and an
  oversized detail being refused.
- [x] **S4 (M) Maintenance repository.** Set (insert or replace), clear, list, load all;
  the cascade on subsystem and route deletion; the unique target index.
  *Done when:* tests cover each scope, replacing a row, the cascades, and rows that no
  longer validate being `Internal`.
- [x] **S5 (S) Actor on revisions and summary-only writes.** `revision.actor`, listed with
  each revision; a summary-only append for token, audit-settings and maintenance changes;
  `restore` leaves tokens, audit and maintenance untouched.
  *Done when:* a test sets maintenance, creates a token, rolls back across them and shows
  both untouched, with every change present in the history naming its actor and no
  secret hash in any revision row.

### Workstream AU: The auth crate (new)

- [x] **AU1 (M) Token format and verification.** Generate `fp_<id>.<secret>` (256 random
  bits, base64url), hash with SHA-256, parse, and verify in constant time; expiry and
  revocation checks take the clock as a parameter.
  *Done when:* tests cover a round trip, a wrong secret, a malformed token, a truncated
  one, expiry at the boundary, revocation, and that the verification does not branch on
  the secret's bytes (the comparison is `subtle`'s constant-time `ct_eq`; `ring`'s own function is deprecated).
- [x] **AU2 (M) The token cache.** An immutable `TokenSet` built from rows (id to hash,
  role, expiry, revocation), swapped atomically, rebuilt reusing unchanged entries when
  the cluster generation moves.
  *Done when:* tests cover a rebuild after create and revoke, an expired token no longer
  authenticating without any write, and 1,000 tokens verifying in microseconds.
- [x] **AU3 (M) The failure throttle.** Per source address, a sliding window of failures
  (10 per minute), `Retry-After` when tripped, bounded memory (an LRU of addresses), the
  clock a parameter.
  *Done when:* tests cover tripping, recovery after the window, the memory bound under a
  flood of distinct addresses, and a valid token being refused while throttled.
- [x] **AU4 (S) WebSocket tickets.** Single-use, 30-second tickets in a bounded in-memory
  map carrying the role.
  *Done when:* tests cover use once, expiry, an unknown ticket, and the bound under
  abuse.
- [x] **AU5 (S) Bypass secrets.** The same hash and constant-time comparison for a
  maintenance bypass header secret, and generation of a secret for the operator.
  *Done when:* tests cover match, mismatch, absent header, and that the stored form is a
  hash.

### Workstream CS: Config service

- [x] **CS1 (L) The actor and the audit transaction.** `ChangeContext { actor, role,
  source, request_id }` becomes part of `apply`; the audit row is written in the same
  transaction as the change and its revision, the revision names its actor, and the
  action name and target come from a small typed `Audited` description the mutation
  returns (so handlers cannot forget). Existing `apply` callers are migrated.
  *Done when:* tests show a committed change always has exactly one audit row and one
  actor on its revision, a vetoed change has neither, and concurrent writes keep their
  own actors.
- [x] **CS2 (M) Token writes and the token set.** Create (returning the secret once),
  revoke and bootstrap through the one writer, each a summary-only revision with an audit
  entry; the in-memory token set is rebuilt on commit and on a sync reload.
  *Done when:* tests cover the last-admin protection, bootstrap only when no admin token
  exists, the environment bootstrap, and two services on one database seeing a new token
  and a revocation within the interval.
- [x] **CS3 (L) Maintenance writes, snapshot input and the sweeper.** Set, update and
  clear a scope through the one writer (summary-only revisions, audit entries); the
  compile step takes the active maintenance rows as an input; a sweeper clears expired
  rows with a normal write (clock passed in); an interval of zero in tests runs it on
  demand.
  *Done when:* tests cover each scope, replacing, cascade on delete, expiry clearing once
  across two services on one database, a restart keeping state and expiry, and a veto
  leaving everything as it was.
- [x] **CS4 (M) Audit maintenance.** Retention pruning on a timer, and the audit entries
  of the sweeper (`system` actor).
  *Done when:* tests cover pruning by age and by count and that pruning never removes the
  newest entries.
- [x] **CS5 (S) Reload.** `reload()` re-reads the database and recompiles unconditionally
  (the sync reload without the version check), returning the live version; shares the
  code path of the sync poll.
  *Done when:* a test edits the database file behind the service's back and `reload()`
  makes it live, and a failing reload leaves the old snapshot.

**PR3 notes (decisions made while building):**
- `apply` takes a `ChangeContext` (who, from where) and an `Audited` (action, target) before the
  mutation, so a write cannot be made unattributed; the audit row is written inside the same
  transaction, the revision names the actor. Admin handlers get the context from a `Change`
  extractor (`anonymous` with the peer address until A1 puts the token there).
- Tokens and maintenance go through `apply` too (a revision with an unchanged configuration copy,
  which bumps the cluster generation); the store's `write_outside_config` was therefore removed.
- CS3's "a veto leaves everything as it was" needs the compile input of **R1** (PR6); the writes,
  the sweeper and the cascade are done and tested here.
- **Q1 moves to PR4**: the HTTP-level harness switch (`TestApp` token helper, `auth = "off"` in the
  binary tests) has nothing to switch until the middleware (A1) exists. Config-service-level
  callers and tests were migrated here.
- C3: `reload_under_load` ran green 10 of 10 in a loop on this PR.

### Workstream R: Routing and compile

- [x] **R1 (M) Compile maintenance into the snapshot.** `Snapshot.maintenance` (global),
  `CompiledSubsystem.maintenance` and `CompiledRoute.maintenance`, each an `Option` of a
  compiled form (the message and body prepared once, the bypass CIDRs parsed, the secret
  hash as bytes, the expiry); a row that references a missing subsystem or route is a
  compile error naming it.
  *Done when:* tests cover each scope, an inactive row being absent, a dangling reference
  being rejected, and unrelated edits keeping runtime state as before.
- [x] **R2 (S) The maintenance decision as one pure function.** `check(scope, now,
  client_ip, header_value) -> Allow | Bypassed | Respond`, in `routing`, shared by the
  data plane and the admin resolver so they cannot disagree (and bypassable for the ACME
  challenge path in M4).
  *Done when:* table tests cover active, expired (at the exact millisecond), each bypass
  rule alone and together, a wrong secret, and an IPv6 address inside and outside a CIDR.

### Workstream D: Data plane

- [x] **D1 (M) The maintenance check.** The pipeline step of 4.2: global and subsystem
  after the 421 and redirect checks, route after matching, before endpoint selection;
  upgrade requests included; the effective client address from `trusted_proxies`; no
  clock read and no allocation when nothing is active.
  *Done when:* end-to-end tests cover each scope, precedence, an unknown host staying 404,
  the redirect still working, a WebSocket handshake getting 503, an established tunnel
  surviving, and no endpoint, retry budget or health state being touched.
- [x] **D2 (M) The response.** 503 with `Retry-After`, the built-in page (escaped),
  JSON by `Accept`, a custom body and content type, `Cache-Control: no-store`, the
  request id, and the access-log field `maintenance=<scope>`.
  *Done when:* tests cover each format, the derived and the configured `Retry-After`, an
  HTML-escaped message containing markup, and the size limit of a custom body.
- [x] **D3 (M) Bypass in the data plane.** CIDR and secret-header bypass, the
  `X-Maintenance-Bypass` response header, and the counters.
  *Done when:* tests cover a bypass by address, by header, a forged `X-Forwarded-For`
  from an untrusted peer not bypassing, a trusted proxy's value being honoured, and a
  bypassed request reaching the upstream.
- [x] **D4 (S) Metrics and the error rate.** The three maintenance metrics of H14, and the
  traffic sampler subtracting maintenance responses from the error rate.
  *Done when:* a scrape after known events asserts each metric, and a burst of
  maintenance responses leaves the dashboard error rate at zero.
- [x] **D5 (S) Health checks continue.** A test that a group in maintenance keeps being
  probed and ejected or recovered, so it is ready on exit.
  *Done when:* the test shows endpoint state changing during maintenance and the group
  serving at once when maintenance ends.

**PR6 notes (decisions made while building):**
- `compile_all` takes `MaintenanceInputs` beside the `Config` (maintenance is not part of it); the old
  `compile` and `compile_with_tls` keep working with none. This also completes CS3: a change vetoed by
  compile leaves the maintenance rows, the snapshot and the audit log as they were (tested).
- The pure decision is `CompiledMaintenance::check`, used by the data plane and by `POST /api/resolve`
  (which gained an optional `clientIp`); `ResolveRequest` is now camelCase.
- `TrustedProxies::effective_client` reads `X-Forwarded-For` from the right, skipping trusted hops, so
  a client cannot write its way onto a bypass network.
- The maintenance 503 is counted in `requests_total` (status class 5xx) like any 503, and also in
  `fastproxy_maintenance_responses_total`; the traffic sampler subtracts the latter from the error rate.
  The access log names the scope (`maintenance=<scope>`).
- The sweeper runs every second (one read of a small table), so the stored state follows the clock
  closely.
- The redirect-before-maintenance and HSTS-with-maintenance cases are tested end to end on the real HTTP
  and HTTPS listeners (`tests/tls_redirect.rs`).

### Workstream A: Admin API and security

- [x] **A1 (L) Authentication and authorization middleware.** The order of 4.1: `Host`
  allow-list, throttle, token or ticket, role per route (every route declares its
  `Action`; a route without one fails a test), request id and the actor into the handler
  context; `/api/health` and the UI assets open; the admin access log names the actor and
  redacts the `/ws` query.
  *Done when:* HTTP-level tests cover 401, 403, a viewer on every write route (generated
  from the router so a new route cannot be forgotten), an expired and a revoked token, a
  bad `Host` (421), and the throttle (429 with `Retry-After`).
- [x] **A2 (M) Tokens API.** `POST`, `GET`, `DELETE /api/tokens`, `GET /api/whoami`;
  the secret only in the creation response; the last admin token protected.
  *Done when:* tests cover create then use, the secret never reappearing (every response,
  event, log line and the metrics text are scanned for it), revoke taking effect on the
  next request, and the 409.
- [x] **A3 (S) WebSocket tickets.** `POST /api/ws-ticket` and ticket authentication on
  `/ws`; a socket with no ticket is refused; viewers connect.
  *Done when:* tests cover a ticket used twice, an expired one, none, and a viewer
  receiving events.
- [x] **A4 (M) Security headers and the key rule.** The headers of H6 on every response
  (CSP checked against the built UI so the UI still runs); the key-bearing routes require
  an admin token and a protected channel (loopback or admin TLS) instead of the bare
  loopback rule.
  *Done when:* tests cover the headers on API, UI and error responses, a key upload from
  a non-loopback peer over plain HTTP refused and over admin TLS accepted, and a viewer
  refused.
- [ ] **A5 (M) Audit API.** `GET /api/audit` with paging and filters, `GET
  /api/audit/export` as NDJSON; denials, failures and authentication events recorded by
  the middleware (no duplicate of a committed change's own entry).
  *Done when:* tests cover each filter, paging, the export, every action name appearing
  once for one change, denials and validation failures appearing, and no secret, key or
  PEM in any entry (a test uploads a certificate and a token and scans the log).
- [x] **A6 (M) Maintenance API.** The routes of 5.3 with validation (`bypass.cidrs`,
  `retryAfterSecs`, `body`, `contentType`), the bypass secret returned once,
  `GET /api/maintenance`, the `maintenanceChanged` event and activity entry; the resolver
  (`POST /api/resolve`) reports whether the host would be in maintenance and why.
  *Done when:* HTTP-level tests cover each scope's set, update and clear, the field
  errors, the secret shown once, the event, the resolver output, and that each write is
  live in the data plane without a restart.
- [x] **A7 (S) Reload and status.** `POST /api/reload`; `GET /api/status` adds the auth
  mode, whether the admin channel is protected, the active maintenance scopes and the
  number of tokens.
  *Done when:* tests cover both and a viewer being refused the reload.
- [x] **A8 (S) Startup rules.** The refusals of 5.1 (non-loopback without TLS and auth,
  `auth = "off"` off loopback), the bootstrap token printed once, and the warnings.
  *Done when:* tests cover every refusal with a message that names the key to set, and
  that the bootstrap token is printed once and never on a second start.

**PR4 notes (decisions made while building):**
- Every admin route is registered through `RouteTable` with an `Access` (`Open`, `Needs(action)` or
  `Ticket(action)`); the per-route guard throttles, authenticates and authorizes, so a route cannot be
  added without saying what it needs, and a test walks the recorded list (viewer on every write route,
  no token on every protected route). The host check and security headers are global layers.
- `PUT /api/certificates/{id}` is `Write`, with the protected-channel rule applied in the handler only
  when the body carries a key, so a rename needs none. The other key routes are `Keys`.
- Until **admin TLS (PR5)**, a non-loopback listener is refused unless `admin.allow_insecure_remote`
  is set, and `auth = "off"` off loopback is refused outright; PR5 replaces the first rule with the TLS
  requirement of H4.
- A bearer token sent to the admin plane is the only credential; the throttle counts failures, not
  successes. The access log names the actor (`actor=<name>`) and redacts the `/ws` query.
- **The UI cannot sign in until PR8** (U1): with authentication on by default it shows errors after
  this PR. For local UI work start with `--admin-auth off` (loopback only). `GET /api/config` is
  protected like the rest.
- Denials, validation failures and authentication events are audited by the middleware in **PR7** (A5);
  committed changes are already audited since PR3.

### Workstream AT: Admin TLS

- [x] **AT1 (L) The admin listener over TLS.** Serve the Axum router through the TLS
  stack with a fixed-certificate resolver reading the live certificate cache by the
  configured name; generate the `admin` self-signed certificate (names from
  `[admin.tls] names`) through the normal write path when absent; handle graceful
  shutdown as the plain listener does.
  *Done when:* tests cover HTTPS and WebSocket on the admin port, the generated
  certificate being served, an uploaded certificate replacing it live, and the plain
  listener not being open when TLS is on.
- [ ] **AT2 (S) The admin certificate in the UI.** The Certificates page shows which
  certificate the admin plane uses, with a note about browser warnings for the generated
  one.
  *Done when:* a UI test covers the card (see U-tasks).

**PR5 notes (decisions made while building):**
- G2 was folded into AT1: the findings (the accept loop, `ConnectInfo` added per request, the
  by-name certificate lookup on every handshake, graceful shutdown) are in ADR 0016.
- The non-loopback rule of PR4 is replaced as planned: a non-loopback `admin.listen` needs
  `admin.tls.enabled` (or the explicit, warned `allow_insecure_remote`); `auth = "off"` off loopback
  stays refused. `--admin-tls` turns it on.
- The certificate the admin plane serves cannot be deleted or renamed (409); replacing its material
  is allowed and is live for the next connection.
- **AT2 (the admin certificate card in the UI) moves to PR8** with U7: it is a UI change. The API
  already reports it (`GET /api/status` -> `admin.tlsCertificate`).

### Workstream CL: The CLI

- [ ] **CL1 (M) Subcommand structure and the admin client.** Restructure the clap
  definition so no subcommand still runs the server; a small client over hyper and
  hyper-rustls with `--url`, token sources (flag, environment, file), `--ca-file` and
  `--insecure`; JSON and plain output; exit codes of H15.
  *Done when:* tests cover the no-subcommand behaviour, token source precedence, an
  unreachable proxy (exit 2), a refused request (exit 1) and TLS to the admin plane with
  a CA file.
- [ ] **CL2 (M) `maintenance`, `reload`, `status`, `audit`.** The operations against a
  running proxy with the scope, TTL, message, retry-after and bypass flags.
  *Done when:* end-to-end tests drive the real binary: maintenance on with a TTL answers
  503 for the host, status shows the countdown, off restores service, `reload` returns
  the version, `audit` shows the change with the actor.
- [ ] **CL3 (M) `config validate`.** Parse and validate the configuration file without
  starting or creating anything, and with `--database` open the file read-only and
  compile the stored configuration (certificates are not decrypted, so TLS inputs are
  inert there, which the output says); every error names the key.
  *Done when:* tests cover a valid file, each validation error with its exit code, a
  database that does not compile, and that no file or directory is created.
- [ ] **CL4 (M) `token create|list|revoke`, offline.** Direct on the database file,
  usable while the proxy runs; `create` prints the secret once; the last-admin
  protection applies; each writes an audit entry with the actor `cli`.
  *Done when:* tests cover recovery (all tokens revoked in the file, a new admin token
  created, the running proxy accepting it within the interval) and the protection.

### Workstream U: Admin UI

- [ ] **U1 (M) Authentication in the client and the sign-in page.** The token in
  `sessionStorage`, sent on every request, a 401 returning to the sign-in page, the
  WebSocket ticket flow, sign-out, `whoami` in the topbar with the role, and the
  read-only mode for a `viewer` (write controls hidden).
  *Done when:* Vitest tests cover sign-in, a rejected token's message, the 401
  redirect, the header on requests, the ticket flow, sign-out clearing the token, and a
  viewer seeing no write control on any page.
- [ ] **U2 (M) Tokens page.** List, create (the secret shown once in a copyable box with
  a plain warning, then gone), revoke with confirmation, the 409 shown, last used.
  *Done when:* tests cover create, the one-time display, revoke, and the 409.
- [ ] **U3 (M) Audit log page.** A paged table with filters (actor, action, outcome,
  time), details, and an export link; live additions from the existing activity events.
  *Done when:* tests cover filters, paging, the empty state and the export.
- [ ] **U4 (L) Maintenance controls.** On the subsystem page (a maintenance section) and
  on each route row: turn on with message, TTL (presets and custom), retry-after, custom
  body, bypass CIDRs and header (the generated secret shown once); a visible state with a
  live countdown; turn off with a confirmation that says what reopens. A global control
  on the new Maintenance page.
  *Done when:* tests cover the forms (validation shown, the exact bodies sent), the
  countdown, the confirmations, the secret shown once, and a viewer seeing state but no
  controls.
- [ ] **U5 (S) Maintenance overview and the banner.** A Maintenance page listing every
  active scope across the proxy with its countdown, and a topbar banner while anything is
  in maintenance, updated live from `maintenanceChanged`.
  *Done when:* tests cover the list, the banner appearing and disappearing, and expiry
  being reflected without a reload.
- [ ] **U6 (M) Routes and upstreams overview.** A read-only cross-subsystem page of routes
  and upstream groups with health and maintenance state, linking to each subsystem.
  *Done when:* tests cover the table, health and maintenance badges, and links.
- [ ] **U7 (S) The admin certificate card** on the Certificates page (AT2) and a note on
  the Settings page about the auth mode and channel protection from `GET /api/status`.
  *Done when:* tests cover both.
- [ ] **U8 (S) Tests and build.** `npm test` and `npm run build` pass; the CSP of A4 is
  checked against the production bundle (no inline script).
  *Done when:* both pass and a test loads the built `index.html` under the CSP.

### Workstream Q: Quality

- [x] **Q1 (M) Test harness migration.** Authentication on in the HTTP-level tests
  (`TestApp` with a token helper, plus a `TestApp::unauthenticated()` where a test is
  about the 401), `auth = "off"` on loopback in the binary tests' generated config, the
  UI mock API sending the header, and the getting-started examples updated. Keeps every
  existing test meaningful (none quietly weakened).
  *Done when:* the whole existing suite passes with authentication on, and a test fails
  if a route is added without an `Action`.
- [ ] **Q2 (L) Exit-criteria end-to-end tests** (`tests/operations.rs`), the real binary:
  authentication required by default with a printed bootstrap token; 401, 403 and the
  viewer role; a token created, used and revoked; recovery with the offline CLI; admin
  TLS; the `Host` check; the throttle; maintenance at each scope with its 503, page,
  `Retry-After`, TTL expiry (waiting on the event, not a fixed sleep), bypass by address
  and by header, persistence across a restart, and health checks continuing; the audit
  trail of all of it with the right actors; the CLI doing the operations. Each must be
  shown to be able to fail.
- [ ] **Q3 (M) Maintenance under load.** Clients keep making requests while maintenance
  is toggled on and off for each scope repeatedly (and TTLs expire): no request fails
  other than with the maintenance 503, a request that began after a toggle returned never
  sees the old state (the sharp rule of `reload_under_load`), no endpoint receives a
  request while its scope is in maintenance, and in-flight requests finish.
- [ ] **Q4 (M) Secret hygiene tests.** The database file (and WAL) contains no token
  secret, no bypass secret and no private key; no response, event, log line, metric,
  audit entry or revision contains a secret after creating tokens, setting bypass secrets
  and uploading a certificate; the `/ws` ticket never appears in the admin access log.
- [ ] **Q5 (M) Cluster behaviour tests.** Two instances (two services, and once two real
  processes) on one database: a token created on one authenticates on the other within
  the interval and a revocation locks it out; maintenance and its expiry follow; the audit
  log is shared with each entry naming its instance; a stale instance cannot write over a
  newer state; a different master key on one instance does not affect tokens (they carry
  no encrypted data).
- [ ] **Q6 (S) Benchmark.** Re-run `docs/benchmarks.md` (M3 baseline) with no maintenance
  active, with maintenance active on an unrelated subsystem, and with maintenance
  answering (the rate of 503s the proxy can serve); record the numbers.
- [ ] **Q7 (S) Gates.** fmt, clippy, tests and UI build green on Linux, macOS and Windows
  on every PR (the M2 and M3 lessons apply: wait for events, not durations; no Unix-only
  commands in tests; one write per raw request).
- [ ] **Q8 (S) Security review.** A checklist run before release: tokens and secrets only
  as hashes and shown once; constant-time comparison; no secret in any output; the
  throttle and `Host` check in force; the CSP real; the loopback and protected-channel
  rule for keys; the refusal to expose an unprotected admin plane; audit entries free of
  secrets and written for denials; maintenance bypass not forgeable through headers; the
  custom body and message not injectable into the page; the last-admin protection; the
  bootstrap token printed once. Findings recorded in `docs/security-review-m5.md`.
- [ ] **Q9 (S) Windows and portability.** The offline CLI, the token file permission
  check (Unix) and the admin TLS tests behave on Windows CI (no certificate files; waits
  on events).

### Workstream W: Documentation

- [ ] **W1 (M) README and getting-started.** A section on authentication (the bootstrap
  token, tokens and roles, the UI sign-in, recovery), admin TLS and the exposure rules,
  maintenance mode (each scope, TTL, bypass, what the response looks like, what it never
  touches), the audit log, the CLI, the config table and the new metrics; an
  **operations** note on running behind a reverse proxy (it makes every request look
  local to the admin plane) and on backing up the database *and* the master key.
- [ ] **W2 (S) AGENTS.md.** The new crate and rules: every admin route declares an
  action; every change goes through `apply` with a `ChangeContext`; tokens, audit and
  maintenance are not `Config`; secrets only as hashes and never in output; maintenance
  is one pure function shared by the data plane and the resolver.
- [ ] **W3 (S) ROADMAP.md.** Tick M5, record decisions H1-H19 in the decision log, update
  the risks table (the admin-exposure risk is closed) and the config example, link this
  plan.
- [ ] **W4 (S) Outcome.** A closing section in this file like M3's section 11.

### Workstream C: Carry-over and housekeeping

- [ ] **C1 (S) Owner click-through of the UI** in a real browser, desktop and phone width,
  light and dark. It was never done for M2 or M3 and M5 adds five pages: do it for the M3
  screens **before U1** so the new work builds on a checked base.
- [ ] **C2 (S) Merge the M3 stack.** Pull requests #29 to #41 are open and stacked; M5's
  branches should be based on `main` after they merge (or stacked on the top of the M3
  stack if merging waits). Decide before the first M5 PR.
- [ ] **C3 (M) The unexplained flaky test.** `reload_under_load` failed once under
  coverage instrumentation and never again in eight loaded runs of the whole suite; keep
  an eye on it, and make the audit-in-transaction change (CS1) not a reason for it to
  reappear: run that test in a loop in CI for the PR that changes `apply`.
- [ ] **C4 (S) The v0.4.0 release.** Run the *Prepare Release* workflow from `main` after
  the M3 merge (the versions are still `0.1.0`, so decide the number: see section 11 of
  M3-PLAN); the M5 release is the next one.
- [ ] **C5 (S) M3 follow-ups worth doing early.** An API and UI to replace a CA bundle
  (the service method exists, there is no endpoint); the UI maintainability findings
  SonarCloud lists on the Certificates page and the resolver (cognitive complexity);
  decide whether certificates and CA bundles should belong to a subsystem (the owner
  proposed and then discarded that during M3; record the decision so it is not
  re-opened by accident).

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| G1 | none (needs the owner's answers) |
| G2 | none (a spike; can start now) |
| G3 | G1 |
| MD1, MD2 | G3 |
| S1 | MD1, MD2 |
| S2, S3, S4 | S1 |
| S5 | S2, S3, S4 |
| AU1 | MD2 |
| AU2 | AU1, S2 |
| AU3, AU4 | AU1 |
| AU5 | AU1, MD1 |
| CS1 | S3, S5 |
| CS2 | CS1, AU2 |
| CS3 | CS1, S4, MD1, R1 |
| CS4 | CS1, S3 |
| CS5 | CS1 |
| R1 | MD1, S4 |
| R2 | R1, AU5 |
| D1 | R2 |
| D2, D3 | D1 |
| D4 | D1 |
| D5 | D1 |
| A1 | CS2, AU2, AU3, Q1 |
| A2 | A1 |
| A3 | A1, AU4 |
| A4 | A1 |
| A5 | CS1, A1 |
| A6 | CS3, A1, D2 |
| A7 | CS5, A1 |
| A8 | A1, CS2 |
| AT1 | G2, A8 |
| AT2 | AT1 |
| CL1 | A1 |
| CL2 | CL1, A6, A7 |
| CL3 | none (uses the config and compile code that exists) |
| CL4 | CS2 |
| U1 | A1, A3, C1 |
| U2 | U1, A2 |
| U3 | U1, A5 |
| U4 | U1, A6 |
| U5 | U4 |
| U6 | U1 |
| U7 | U1, AT2 |
| U8 | U1-U7 |
| Q1 | MD2 (the harness can be migrated before the middleware exists, behind a switch) |
| Q2 | A1-A8, AT1, CL1-CL4, D1-D5, CS1-CS5 |
| Q3 | D1-D3, A6 |
| Q4 | CS1, CS2, CS3, A2, A5 |
| Q5 | CS2, CS3, AU2 |
| Q6 | D1, D4 |
| Q7 | every code task (runs in CI on each PR) |
| Q8 | every code task |
| Q9 | CL4, AT1 |
| W1-W4 | feature tasks complete |
| C1 | none (the owner's check; do it before U1) |
| C2 | none (a decision before the first M5 PR) |
| C3-C5 | none |

Suggested slicing into pull requests, each leaving `main` green and releasable. **Merge
each to `main` as it passes CI** so the stack stays short (the M3 stack of eleven open
pull requests is the lesson):

1. **PR1** G1, G3 (decisions, ADRs 0015 to 0019, defaults); G2 as its own throwaway
   branch.
2. **PR2** MD1, MD2, S1-S5, AU1-AU5 (the model types, the schema, the repositories, the
   auth crate): no behaviour yet.
3. **PR3** CS1-CS5 and Q1 (the actor and audit transaction, tokens, maintenance writes and
   the sweeper, reload, and the test harness moved to authentication behind a switch):
   behaviour unchanged for users; this is the PR that changes `apply`, so the
   `reload_under_load` loop of C3 runs on it.
4. **PR4** A1-A4, A7, A8 (authentication, roles, tokens, tickets, headers, the `Host`
   check, throttle, startup rules, reload and status): the admin plane is protected.
5. **PR5** AT1, AT2 (admin TLS and the admin certificate).
6. **PR6** R1, R2, D1-D5, A6 (maintenance end to end: compile, data plane, bypass, metrics,
   API and events).
7. **PR7** A5, CL1-CL4 (the audit API and the CLI).
8. **PR8** U1-U3, U7 (UI: sign-in, tokens, audit, settings and the admin certificate card),
   after the owner's C1 click-through.
9. **PR9** U4-U6, U8 (UI: maintenance controls, overview and banner, routes overview).
10. **PR10** Q2-Q9, W1-W4, C3-C5 (the exit-criteria tests, the benchmark, the security
    review, the documentation and the outcome).

PRs 4 and 6 are independent after PR3 and can be taken in either order; PR5 needs PR4;
PR7 needs PR3 and PR6; the UI pull requests need their APIs.

## 8. Performance and cost budget

- **No maintenance active must not slow down the data plane:** within about 5% of the M3
  numbers in [benchmarks.md](benchmarks.md) (one endpoint, 64 connections: about 97k rps
  with TLS off, about 94k with TLS on). The new step is a read of three `Option`s that are
  `None`: no clock read, no allocation, no atomic.
- **Maintenance active on another subsystem** must cost the unaffected subsystems nothing
  measurable (the check is per matched subsystem and route).
- **A maintenance response** is built from prepared bytes and should be served at a rate
  close to the fastest fixed responses (a 404 today); record it. It must not touch the
  upstream pool, so a maintenance flood cannot exhaust connections to a backend.
- **Authentication** adds one `SHA-256` and a table lookup per admin request; the admin
  plane is not a hot path, but 1,000 tokens must still verify in microseconds and a
  rebuild of the token set must not stall requests.
- **The audit write** is one extra `INSERT` in the transaction that already commits a
  change: a configuration write should stay within the M3 figure (about half a
  millisecond for a certificate write) plus well under a millisecond.
- **Retention** pruning runs hourly and in batches, so it never holds the writer lock for
  long; 200,000 rows prune in well under a second.
- **Memory:** the throttle, tickets and token set are bounded (an LRU of addresses, a
  capped ticket map, tokens are rows).

## 9. Risks specific to M5

| Risk | Mitigation |
|---|---|
| **Locking everyone out** (a lost token, a revoked last admin, a wrong role change) | The last usable admin token cannot be revoked, expired or demoted (409); a bootstrap token on first start; the offline CLI creates a new admin token from the machine; tested end to end (Q2, CL4) |
| **A secure default breaks every existing workflow** (tests, the UI dev server, curl examples, the benchmark scripts) | One explicit switch for loopback development (`auth = "off"`, warned), the harness migration is its own task (Q1) done before the middleware lands, the docs and scripts are updated in the same PRs |
| **The admin plane still exposed through a local reverse proxy** (every request looks like loopback) | The key-upload rule now depends on a protected channel plus an admin token, not on the peer address alone; the `Host` allow-list and authentication apply regardless; the README says plainly that a reverse proxy in front needs its own access control |
| **Token theft from the UI** (the token lives in the browser) | `sessionStorage` only, a strict CSP with no inline script or third-party origins, `no-store`, a sign-out, token expiry and instant revocation; the risk of an XSS bug is stated in the sign-in page and the ADR |
| **A bearer token sent over plain HTTP** | Non-loopback binding requires admin TLS; a token over plain loopback never leaves the machine; the API refuses key-bearing requests on an unprotected channel |
| **Audit entries leaking secrets, or growing without bound** | Entries are built from names and ids only (a typed `Audited` description, never a body), tests scan the log for secrets, retention by age and count with batched pruning; the detail field is size-capped |
| **Audit entry and change drifting apart** (a change without its entry, or an entry for a change that was vetoed) | The entry is written in the same transaction as the change and its revision (CS1); failures and denials, which have no transaction, are recorded separately and never as a committed change |
| **Maintenance left on by mistake** | A TTL is offered by default in the UI and the CLI, a banner shows on every page while it is active with a countdown, the dashboard and metrics expose it, and expiry is exact because it is checked at request time |
| **Clock skew between instances making maintenance end at different moments** | Documented; the expiry is absolute wall-clock time and instances are expected to run NTP; the window is the skew, and the sweeper makes the database consistent afterwards |
| **A maintenance bypass that can be forged** | The effective client address honours `X-Forwarded-For` only from trusted proxies; the secret header is compared as a hash in constant time; tested with a forged header from an untrusted peer (D3, Q8) |
| **Operator text injected into the public maintenance page** | The built-in page escapes the message; a custom body is the operator's explicit choice, size-capped and limited to three content types; tested with markup (D2) |
| **Maintenance changing the meaning of rollback** | Maintenance, tokens and audit are outside `Config` and outside rollback (H8): rolling back never switches maintenance on or off, revokes a token or erases history |
| **`apply` is touched by almost every admin handler** (the actor) | One typed `ChangeContext` and one audited-description type, migrated in one PR with the harness (CS1, Q1), with the `reload_under_load` loop of C3 as the guard |
| **Admin TLS adds a chicken-and-egg** (the certificate is managed through the plane that needs it) | A self-signed `admin` certificate is generated on first need through the normal write path, replaceable live; the plain listener is never opened when TLS is on; spike G2 proves the stack first |
| **The WebSocket ticket in a URL** | Single-use, 30 seconds, in memory, redacted from the access log; a viewer's socket carries only non-secret events |
| **Scope creep** (accounts, SSO, schedules, an alerting system) | Section 2 lists what is out; the roadmap, not this plan, takes new ideas |
| **Interplay with M4 landing later** | Section 11 pins the touch points (the ACME challenge bypass, the admin certificate, audit actions, the schema), so M4 does not reopen M5's design |

## 10. Definition of done for M5

- [ ] Every task above is checked, or explicitly moved with a note.
- [ ] Exit criteria (section 1) demonstrated by the end-to-end tests in Q2 to Q5 and a
  manual run: start fresh and copy the bootstrap token, sign in to the UI, create a viewer
  token and see the read-only UI, turn maintenance on for a route with a TTL and a bypass
  header, see the 503 and the banner, verify through the bypass, watch it expire, read the
  audit trail, revoke the token, lose every token and recover with `token create`, put the
  admin plane behind TLS, and watch a second instance follow a token and a maintenance
  toggle.
- [ ] CI green on all three platforms; no new clippy warnings.
- [ ] Overall test coverage above 60%, number recorded (AGENTS.md rule); aim to stay near
  M3's 97.3%.
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [ ] README, AGENTS.md, ROADMAP.md and getting-started updated, including the exposure
  rules and the operations notes.
- [ ] Benchmark re-run and recorded; the idle overhead of maintenance within budget.
- [ ] The security review (Q8) done, findings recorded, secret hygiene verified (Q4).
- [ ] The owner's click-through of the UI done (C1) and the M3 stack merged (C2).

## 11. What M5 leaves for M4, and the other touch points

M4 (ACME) will be planned and built later. These constraints are fixed now so it does
not reopen M5:

- **The HTTP-01 challenge must bypass maintenance and the HTTPS redirect.** Both are one
  pure function each (`routing::check` for maintenance, `redirect_target` for the
  redirect); M4 adds the challenge path as an exemption in the one place before them, so a
  proxy in maintenance still renews its certificates.
- **ACME writes through the same path as everything else**: `ConfigService::apply` with a
  `ChangeContext` whose actor is `system` (the renewal task) and audit actions
  `certificate.acme.issue` and `certificate.acme.renew`; the audit log therefore shows
  every automatic issuance without M4 touching the audit code.
- **The admin certificate may be ACME-issued** for a public admin name: it is a stored
  certificate chosen by name (H4), so M4's per-subsystem ACME is not needed for it, and
  the admin listener follows a renewal through the same certificate cache.
- **The ACME account key is sealed by the vault** (M3 ADR 0014) and is, like tokens and
  maintenance, **not** part of `Config` or of rollback.
- **Schema:** M5 takes schema version 4. M4 adds its columns and tables by editing the
  schema in place again (no migrations while undeployed), so the order of M4 and M5 does
  not matter for the schema; whoever lands second edits the file and bumps the version.
- **Cluster:** an ACME lease and the token set both ride the revision-version generation
  that the sync poll already follows; nothing in M5 needs a second mechanism.
- **Roles:** M4's "ACME on" per subsystem is a write like any other: `admin` only; a
  `viewer` sees the status.

Other touch points with finished work:

- **M3 certificates** are written by tokens now: key-bearing routes need an `admin` token
  and a protected channel; the loopback rule (F27) is superseded by that (H4), and its
  tests move to the new rule.
- **M3 cluster sync** carries tokens and maintenance (summary-only revisions); the poll,
  the vault check and the `Prepared` state are reused, not duplicated.
- **M2 endpoint drain and disable** stay the tool for taking a *backend* out;
  maintenance is for taking a *site* out with an explanation. The docs say which to use.
