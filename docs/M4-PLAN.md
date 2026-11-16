# M4 Plan: ACME automatic certificates

Status: **v1.0 (2026-11-16), complete; PR1 (#68), PR2 (#69), PR3 (#70) and PR4 (`m4/pr4-surface`) written, awaiting
the owner's review and manual check.** The outcome is in section 12.
The owner asked to start the work, so PR1 builds on the recommendations as written; the ADRs (0024 to 0027) stay
*proposed* until the owner confirms the **(confirm)** items, and anything changed at review is reworked there.

Parent document: [ROADMAP.md](ROADMAP.md) (section 4.4 and the M4 list). M4 was skipped after M3 and is planned
now, after M5 and M6. Its design constraints were fixed early and are binding here: M3 F29 and
[ADR 0014](adr/0014-acme-constraints-for-m4.md), M5 section 11 (the challenge bypasses maintenance and the
redirect; issuance is audited as `system`; the account key is sealed and is not `Config`), and M6 section 11 (the
challenge path is not sampled). Task sizes are relative effort (S under half a day, M about 1-2 days, L about 3-5
days), not commitments. Tick the checkboxes as tasks land.

## 1. Goal and exit criteria

Today an operator who turns HTTPS on for a subsystem gets a self-signed development certificate, then must buy or
obtain a real certificate, upload it, and remember to replace it before it expires. M4 makes the proxy do that
itself: **a subsystem with ACME on gets a publicly trusted certificate for its host names, issued and renewed
automatically through an ACME CA (Let's Encrypt by default), with no restart and no dropped connection**, in a
single instance and in a cluster of instances sharing one database.

Everything M3 built stays the delivery path: an issued certificate is a stored certificate (`source = acme`), its
key is sealed by the vault, it is written through `ConfigService::apply`, it travels in the routing snapshot, and
other instances pick it up through the sync poll. M4 adds the parts that are new: an ACME account, the order and
challenge flow, a renewal scheduler, a lease so only one instance talks to the CA, and the status to see it all.

**Exit criteria** (from the roadmap, made measurable):

1. **Issuance works end to end.** With ACME on for the process and for a subsystem, the proxy registers an account
   (once), orders a certificate for the subsystem's eligible host names, answers the HTTP-01 challenge on its plain
   HTTP listener, finalizes, stores and attaches the certificate, and the **next TLS handshake** presents it. The
   development certificate serves until then. Tested against an in-process ACME test CA that really fetches the
   challenge from the proxy, and (opt-in, Linux CI) against Pebble.
2. **Domains follow the database.** Adding an eligible host to an ACME subsystem triggers a new order that covers
   it (debounced, so a bulk edit makes one order); a wildcard host, an IP literal, a single-label name, or a name
   outside the allowlist is **skipped with a visible reason**, never ordered.
3. **Renewal is automatic and safe.** A certificate is renewed at about two thirds of its lifetime (jittered); a
   failed attempt backs off (honouring `Retry-After` and rate-limit errors) and is retried; **the current
   certificate keeps serving** through any failure; the status shows the next attempt and the last error.
   Tested with an injected clock and a test CA that issues short-lived certificates.
4. **The challenge is answered ahead of everything else.** A request for
   `/.well-known/acme-challenge/<token>` with a known token is answered by the proxy even when the subsystem
   redirects to HTTPS, is in maintenance, or is disabled; an unknown token falls through to normal routing (so a
   backend that runs its own ACME is not broken); the answer is never forwarded, sampled or retried.
5. **One instance talks to the CA.** With two instances on one database, exactly one holds the ACME lease and
   places orders; **either instance answers the challenge**; the issued certificate reaches both through the sync
   poll; when the lease holder stops, the other takes over after the lease expires and finishes the work.
6. **Restarts and rate limits are respected.** A restart never re-orders a certificate that is valid and not due;
   the account is reused; per-subsystem failures are bounded per hour; the default directory is **Let's Encrypt
   staging**, and production must be chosen explicitly.
7. **No secret leaks.** The account key and every issued private key are sealed by the vault before they reach the
   store; neither appears in a log line, metric, audit entry, revision, event, API response or the database file
   unsealed (`tests/secret_hygiene.rs` is extended, and a scan of the database and its WAL finds no PEM key).
8. **It is visible and audited.** Each issuance, renewal and failure is an audit entry with actor `system`
   (`certificate.acme.issue`, `certificate.acme.renew`, `certificate.acme.fail`); the admin API, the CLI and the
   UI show per-subsystem ACME status (names, state, expiry, next attempt, last error, which instance holds the
   lease); metrics count orders by outcome and challenge requests by outcome; an operator can force a renewal.
9. **No regression:** the M1, M2, M3, M5 and M6 exit criteria still pass; the data plane still never touches the
   database and loads the snapshot once per request; the cost of the challenge check on a normal request is a
   prefix comparison (within the noise of [benchmarks.md](benchmarks.md)).

## 2. Scope

### In scope

- `[data_plane.acme]` in `config.toml` and the `--acme` / `--no-acme` flags.
- A new crate `fast-proxy-acme`: the protocol client adapter, the pure renewal and eligibility policy, the order
  state machine. A new crate `fast-proxy-acme-service`: the runner that holds the lease, drives orders and writes
  results through `ConfigService`.
- The ACME account (one per directory), sealed by the vault; per-order ECDSA keys and CSRs generated in
  `fast-proxy-tls`.
- The per-subsystem switch `https.acmeEnabled`, accepted only when ACME is on for the process.
- HTTP-01 challenges: tokens stored in the database, compiled into the snapshot, answered by the data plane on every
  instance ahead of the redirect, the 421 rule and maintenance.
- The lease, the per-subsystem status table, the scheduler (jitter, backoff, rate-limit handling, debounce).
- Admin API, a WebSocket event, CLI (`acme status`, `acme renew`), UI (status on the subsystem page and an ACME
  overview), metrics, audit actions.
- An in-process ACME test CA crate for tests; an opt-in Pebble job in CI.
- Tests, ADRs, docs, a security review section for ACME.

### Out of scope (later milestones, or never)

| Item | Where |
|---|---|
| DNS-01 and wildcard certificates | post-1.0 (DNS provider integrations); wildcard hosts are skipped with a reason |
| TLS-ALPN-01 | stretch (task X1), only after the exit criteria hold |
| ACME Renewal Information (ARI, RFC 9773) | stretch (task X2); the fixed fraction of lifetime is the rule until then |
| External Account Binding (ZeroSSL, Google Trust Services, corporate CAs that need it) | later; the config section leaves room for it (`eab_*` keys rejected as unknown today) |
| An ACME certificate for the **admin** listener | later: M5 H4 already lets the admin listener serve any stored certificate by name, so an operator can point it at `acme-<subsystem>`; issuing for an admin-only name needs its own trigger |
| RSA certificate keys | not planned: `ring` cannot generate RSA keys; ECDSA P-256 (default) and P-384 are offered |
| Revoking certificates through ACME | later; deleting the certificate locally is enough for a proxy |
| OCSP stapling, must-staple | not planned (Let's Encrypt ended OCSP in 2025) |
| Per-subsystem directories or accounts | not planned: one directory per process, from the config file |
| Configuration import/export (M5 carry-over) | stays open; not taken by M4 |

## 3. Design decisions for M4

Items marked **(confirm)** are recommendations that need your sign-off.

| # | Decision | Recommendation |
|---|---|---|
| K1 | ACME protocol client | Use **`instant-acme`** (async, RFC 8555, rustls, a `ring` feature so no second crypto stack) behind a narrow trait, `AcmeClient`, in `fast-proxy-acme`, so the scheduler and the tests never see the library. Writing our own JWS and order flow is about 1 000 lines of protocol code we would have to fuzz and keep current; the library is used in production by several proxies. Spike G2 confirms the dependency tree (no `aws-lc-rs`, one `hyper`/`rustls` version), a Windows build, serialisable account credentials, and a run against Pebble. Fallback if the spike fails: our own minimal client on `ring` + `hyper-rustls` behind the same trait. **(confirm)** |
| K2 | Crate layout | **`fast-proxy-acme`** (depends on `model` only, plus the client library): `AcmeClient` and its adapter, the pure `RenewalPolicy` (when is a certificate due, jitter, backoff), `eligible_names` (which hosts can be certified and why not), and the `OrderStep` state machine; time is a parameter. **`fast-proxy-acme-service`** (depends on `acme`, `config-service`, `tls`, `model`): the runner that takes the lease, schedules, drives orders and writes every result through `ConfigService`. **`fast-proxy-tls`** gains key and CSR generation (`acme_key.rs`, `rcgen` is already a dependency) and a general `Vault::seal_secret` for the account credentials. `routing` compiles the challenge map into the snapshot; `dataplane` answers from it. Dependencies still point one way; the data plane depends on neither new crate, and the store stays free of cryptography. **(confirm)** |
| K3 | Process settings | `[data_plane.acme]` in `config.toml` (restart to change): `enabled` (`--acme`/`--no-acme`), `directory` (default **Let's Encrypt staging**; production must be written out), `email` (optional: Let's Encrypt no longer sends expiry mail, other CAs may want it), `allowed_domains` (patterns; **required and non-empty** when enabled, so a mistyped host cannot spend rate limit on a name nobody meant), `key_type` (`ecdsa-p256` default, `ecdsa-p384`), `directory_ca` (an optional PEM file to trust the directory, for Pebble and private CAs such as step-ca), `renew_check_interval` (default 1 h), `propagation_wait` (default derived, K8), `lease_ttl` (default 10 min). ACME needs TLS: `acme.enabled` with `tls.enabled = false` refuses to start, as does ACME with no plain-HTTP listener; a plain listener not on port 80 starts with a warning (NAT is legitimate). Starting against a staging directory logs a warning that the certificates are not trusted. **(confirm)** |
| K4 | The per-subsystem switch | `https.acmeEnabled` (column `https_acme_enabled`), part of the subsystem's HTTPS settings and therefore of `Config` like the rest of them. Accepted **only when ACME is on for the process** (400 on `https.acmeEnabled` otherwise) **and `https.enabled` is true**; inert (stored, status `inactive`) when the process switch is later off (ADR 0014). It excludes a manually attached certificate: turning ACME on is allowed with any current certificate, and the ACME certificate **replaces the attachment** when it is issued; attaching another certificate while ACME is on is refused (400 naming the field); turning ACME off keeps the last ACME certificate attached until the operator changes it. `admin` only, like every write. **(confirm)** |
| K5 | Which names are certified | One certificate per subsystem, named `acme-<subsystem>`, covering all its **eligible** host patterns: exact names (no wildcard), at least two labels, matched by `allowed_domains`, at most 100 names (the Let's Encrypt limit; further names are skipped with a reason). Host patterns can never be IP literals, so there is no reason for them. Each skipped name and its reason (`wildcard_needs_dns01`, `single_label`, `not_allowed`, `too_many_names`) is in the status. A subsystem with no eligible name is `idle` with that reason. The rule is one pure function used by the runner, the API (to show what *would* be ordered) and the UI. |
| K6 | When an order is placed | (a) **Due:** the attached ACME certificate has passed two thirds of its lifetime (for a 90-day certificate, about day 60), with a stable per-subsystem jitter of up to 5% of the lifetime so a fleet does not renew in one minute; (b) **names changed:** the eligible set is not a subset of the certificate's names (a host was added), debounced 30 s after the last change; a removed host does not force an order, the next renewal drops it; (c) **missing:** ACME is on and no ACME certificate is attached; (d) **forced:** an admin asked (K13). The runner checks at `renew_check_interval` and on every `ConfigChanged`, and computes "due" only from stored facts (certificate `not_after`, the status row), so a **restart never re-orders** a valid certificate. **(confirm)** |
| K7 | Backoff and rate limits | Per subsystem, after a failure: 1 min, 5 min, 30 min, 2 h, then every 6 h, with jitter; a `Retry-After` from the CA or a `rateLimited` error sets the next attempt to the time it names (or 1 h if none) and is shown as `rate_limited`. At most **5 failed orders per subsystem per rolling hour** whatever the backoff says; a `badDomain`, `rejectedIdentifier`, `caa` or `dns` error (the name can never validate as configured) waits 6 h straight away. `next_attempt_at` and the failure count are stored, so a restart does not reset the backoff. The current certificate keeps serving throughout. The policy is pure and property-tested. **(confirm)** |
| K8 | Challenge tokens and propagation | HTTP-01 only. The runner writes the pending challenges of one order (token, key authorization, the names it is for, an expiry of 1 h) to the `acme_challenge` table through `ConfigService::apply` as a **summary-only revision** (like tokens and maintenance: not `Config`, not rolled back), so the challenges are compiled into the snapshot of the writing instance at once and of every other instance through the sync poll. Before telling the CA to validate, the runner waits **`propagation_wait`** (default `2 x config_poll_interval + 2 s`, 0 with the poll off), then runs a **self-check**: it fetches its own challenge URL through the public name a few times (an instance behind a load balancer may reach a sibling, which is the point); a failing self-check fails the order locally with a clear error and **spends no CA attempt**. The challenges of an order are removed in the same apply that stores the certificate, or on failure, so one order costs **two revisions** (publish, then finish). The key authorization is public by design (the CA fetches it over plain HTTP), so it is not a secret and may sit in the snapshot. **(confirm)** |
| K9 | Answering the challenge | In the data plane, **first** in `resolve`, before the redirect, the 421 rule and maintenance (M5 section 11): if the path is `/.well-known/acme-challenge/<token>` and the snapshot has that token **for that host**, answer `200 text/plain` with the key authorization. Any method other than `GET`/`HEAD`, an unknown token or a host the token is not for **falls through to normal routing**. Answered on plain HTTP and on HTTPS (the CA only uses port 80, but a redirect-following validator is harmless). The answer is never forwarded, never sampled (M6 section 11), never counted as a route; it has its own metric. The check costs one prefix comparison on a normal request. **(confirm)** |
| K10 | The lease (one instance talks to the CA) | One row, `acme_lease (holder, expires_at)`, taken and renewed by compare-and-swap in one transaction (`UPDATE ... WHERE holder = me OR expires_at < now`), renewed every `lease_ttl / 3`; the runner works only while it holds it and stops placing new orders when a renewal of the lease fails. A lost lease mid-order is safe: the new holder sees the stored status and starts the order again (an abandoned ACME order simply expires at the CA). The lease, like the status (K11), is **coordination state, not configuration**: it is written **without a revision** (otherwise every lease renewal would make every instance reload), through a narrow `ConfigService` method so `ConfigService` stays the only writer; this is the second deliberate exception after M6 samples (ADR 0022) and gets ADR 0026. The holder's instance id is in the status. **(confirm)** |
| K11 | Status | One row per subsystem with ACME on, `acme_status`: state (`idle`, `pending`, `valid`, `renewing`, `failed`, `rate_limited`, `inactive`), the eligible and skipped names, the attached certificate, `next_attempt_at`, the failures since the last success (the backoff) and in the
current hour (the cap), the last error (a sanitized CA problem type and a capped, single-line detail, never a URL with a token or a key), the last attempt and success times, and the instance that made them. Written without a revision (K10); the admin plane reads it from the store directly, so every instance shows the same status. Every change also goes out as a WebSocket `acme_status` event on the instance that made it. |
| K12 | Account | One account per directory URL, created on the first order (not at startup, so turning ACME on with no subsystem using it contacts nobody). The **whole credentials blob** (account URL and private key) is sealed by the vault (`Vault::seal_secret`) and stored in `acme_account (directory, email, credentials_enc, credentials_nonce, created_at)`. Changing the directory makes a new account; changing the email updates the account contact on the next order. Account and challenges are not `Config` and never rolled back (M5 section 11). Accepting the CA's terms of service is implied by `enabled = true`; the docs say so and the startup log names the directory. **(confirm)** |
| K13 | Issued keys and the write | A **fresh key per order** (ECDSA P-256 by default) and a CSR are generated in `fast-proxy-tls`; the key is sealed before the write; the certificate is stored with `source = acme` under `acme-<subsystem>`, **replaced in place** on renewal (same id, so the snapshot swap and the certificate cache recycle of ADR 0011 apply unchanged), attached to the subsystem, and the challenges cleared, all in **one** `ConfigService::apply` with actor `system` and audit action `certificate.acme.issue` or `certificate.acme.renew`. The new chain is validated with the same code as an upload (it must cover the names, match the key, be currently valid) before it is written; a CA response that fails validation is a failed order. The unattached `dev-<subsystem>` certificate generated for that subsystem by M3 is deleted in the same write (it was the proxy's own and nothing else uses it); an uploaded certificate that was attached is left in the store, unattached. **(confirm)** |
| K14 | Forced renewal and access | `POST /api/subsystems/{id}/acme/renew` (action `Write`, `admin`) puts the subsystem first in the queue; it is refused with 409 while an order for it is running and with 429 when the per-hour failure cap (K7) is reached; audited (`acme.renew.request`). Status reads are `Read` (a `viewer` sees status, never a key). No new `Action` is needed. |
| K15 | Clock | Due dates compare wall-clock certificate times, so the runner reads the wall clock once per pass and passes `now_ms` into the pure policy (the M6 pattern); tests drive it with a fake clock and a test CA that issues certificates lasting minutes. |
| K16 | Testing the protocol | A new dev-only crate, **`fast-proxy-test-acme`**: an in-process ACME server on axum implementing the subset the client uses (directory, nonce, account, order, authorization, HTTP-01 challenge **validated by really fetching** from the proxy, finalize, certificate download) and issuing from the in-memory CA of `fast-proxy-test-pki`, with switches for failure, `rateLimited`, slow validation and short lifetimes. It runs on all three platforms. A separate CI job on Linux runs the same end-to-end test against **Pebble** (downloaded release binary, `#[ignore]` unless `FAST_PROXY_PEBBLE` is set) to catch disagreement between our fake and a real implementation. **(confirm)** |
| K17 | Compatibility | None kept: the project is undeployed. The schema is edited in place (version 5 to 6), `SCHEMA_VERSION` bumped. |

## 4. Target architecture

```
 config.toml [data_plane.acme] ──► fast-proxy-acme-service (runner, one per instance)
                                       │  holds the lease? ── no ──► idle, re-check every lease_ttl/3
                                       │  yes
                                       ▼
            ┌─────────── fast-proxy-acme ───────────┐
            │ RenewalPolicy (due? backoff? jitter)  │  pure, now_ms in
            │ eligible_names (K5)                   │
            │ AcmeClient ─► instant-acme ─► CA ◄────┼──── HTTP-01 fetch ──┐
            └───────────────────────────────────────┘                     │
                                       │                                   │
          every write through ConfigService (actor `system`)               │
           ├─ lease / status ......... no revision (K10, K11)              │
           ├─ challenges ............. summary-only revision (K8)          │
           └─ certificate + attach ... revision + audit, sealed key (K13)  │
                                       │                                   │
             sync poll (other instances) ─► snapshot swap                   │
                                       ▼                                   │
     data plane: resolve() ─► [ACME challenge?] ─► redirect ─► 421 ─► maintenance ─► routing
                                  └── answer from Snapshot.acme_challenges ◄┘   (never the database)
```

### 4.1 One issuance, in order

1. The runner holds the lease and finds subsystem `shop` due (K6), with eligible names `shop.example.com`,
   `www.shop.example.com`.
2. It loads (or creates and seals) the account, creates an order, and reads the two HTTP-01 challenges.
3. One apply writes both challenges (summary-only revision); the local snapshot carries them at once.
4. It waits `propagation_wait` for siblings to sync, then self-checks each challenge URL.
5. It tells the CA the challenges are ready and polls the order (with the CA's `Retry-After`) until it is ready.
6. It generates a key and CSR, finalizes, downloads the chain, validates it against the key and names.
7. One apply: seal the key, replace `acme-shop` in place (or create it), attach it, clear the challenges,
   delete the unused `dev-shop`, audit `certificate.acme.issue`. The snapshot swaps; the next handshake uses it.
8. The status row becomes `valid` with the next due date; an `acme_status` event goes out.

On any failure from step 2 to 7: the challenges are cleared, the status records the sanitized error and the next
attempt (K7), an audit entry `certificate.acme.fail` is written, and the old certificate keeps serving.

### 4.2 Several instances on one database

The lease makes one instance the only one that talks to the CA; the challenges and the certificate reach every
instance through the revision-version sync poll that M3 built; the status is read by every admin plane from the
shared table. Nothing ACME needs is held only in one instance's memory, so the lease can move at any time. What
stays local: the HTTP client to the CA and the in-flight order (lost with its instance and restarted by the next
holder).

## 5. Configuration and data model changes

### 5.1 Config file (`config.toml`) and command line

```toml
[data_plane.acme]
enabled = false            # --acme / --no-acme; needs [data_plane.tls] enabled and a plain listener
directory = "https://acme-staging-v02.api.letsencrypt.org/directory"   # production must be written out
# email = "ops@example.com"
allowed_domains = []       # required when enabled, e.g. ["*.example.com", "example.org"]
key_type = "ecdsa-p256"    # or "ecdsa-p384"
# directory_ca = "/etc/fast-proxy/pebble-ca.pem"   # trust a private or test directory
renew_check_interval = "1h"
# propagation_wait = "12s" # default: 2 x config_poll_interval + 2s
lease_ttl = "10m"
```

`allowed_domains` uses the same pattern syntax as host patterns: `example.com` allows exactly that name and
`*.example.com` any name below it (not `example.com` itself). Unknown
keys are rejected, as everywhere in the file.

### 5.2 Database (schema 5 to 6, edited in place; no migration)

```sql
-- subsystem gains:
  https_acme_enabled INTEGER NOT NULL DEFAULT 0,

-- One account per directory; the credentials (account URL and key) are sealed by the vault.
CREATE TABLE acme_account (
  id                INTEGER PRIMARY KEY,
  directory         TEXT NOT NULL UNIQUE,
  email             TEXT NOT NULL DEFAULT '',
  credentials_enc   BLOB NOT NULL,
  credentials_nonce BLOB NOT NULL,
  created_at        INTEGER NOT NULL
);

-- Pending HTTP-01 challenges; not Config, compiled into the snapshot.
CREATE TABLE acme_challenge (
  token        TEXT PRIMARY KEY,
  subsystem_id INTEGER NOT NULL REFERENCES subsystem(id) ON DELETE CASCADE,
  names        TEXT NOT NULL,      -- JSON array: the hosts this token answers for
  key_auth     TEXT NOT NULL,
  expires_at   INTEGER NOT NULL
);

-- One row: who may talk to the CA. Written without a revision.
CREATE TABLE acme_lease (
  id         INTEGER PRIMARY KEY CHECK (id = 1),
  holder     TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Per-subsystem status. Written without a revision.
CREATE TABLE acme_status (
  subsystem_id    INTEGER PRIMARY KEY REFERENCES subsystem(id) ON DELETE CASCADE,
  state           TEXT NOT NULL,
  eligible_names  TEXT NOT NULL,   -- JSON array
  skipped_names   TEXT NOT NULL,   -- JSON array of {name, reason}
  certificate_id  INTEGER REFERENCES certificate(id) ON DELETE SET NULL,
  next_attempt_at INTEGER,
  failures        INTEGER NOT NULL DEFAULT 0,   -- since the last success: the backoff
  window_failures INTEGER NOT NULL DEFAULT 0,   -- since failure_window_start: the hourly cap
  failure_window_start INTEGER,
  last_error      TEXT NOT NULL DEFAULT '',
  last_attempt_at INTEGER,
  last_success_at INTEGER,
  instance        TEXT NOT NULL DEFAULT '',
  updated_at      INTEGER NOT NULL
);
```

The SQL stays portable (millisecond `INTEGER` times, JSON as text, `BLOB`) for the later PostgreSQL project.
Expired challenges are removed by the runner and ignored by compile.

### 5.3 API shape (sketch)

```
PUT  /api/subsystems/{id}/https           + "acmeEnabled": true           (Write, existing route)
GET  /api/acme                            process: enabled, directory, staging, account registered,
                                           lease holder and expiry, allowedDomains; and every subsystem's status  (Read)
GET  /api/subsystems/{id}/acme            one status, plus the names that would be ordered now (K5)          (Read)
POST /api/subsystems/{id}/acme/renew      force an order now (K14)                                           (Write)
WS   acme_status                          { subsystem, state, notAfter, nextAttemptAt, lastError }
```

CLI: `fast-proxy acme status [--json]` and `fast-proxy acme renew <subsystem>`, with the M5 exit codes.

Metrics (labels from configuration names and fixed enums only): `fastproxy_acme_orders_total{subsystem,outcome}`
(`issued`, `renewed`, `failed`, `rate_limited`, `self_check_failed`), `fastproxy_acme_challenge_requests_total{outcome}`
(`answered`, `unknown_token`), `fastproxy_acme_lease_held` (0/1), `fastproxy_acme_next_attempt_seconds{subsystem}`.
Expiry is already `fastproxy_tls_cert_expiry_seconds{cert,source="acme"}` from M3.

## 6. Tasks

Workstream letters: **G** groundwork, **MD** model, **S** store, **F** config file, **T** the TLS crate, **AC**
the ACME crate, **TA** the test CA, **CS** config service, **R** routing, **D** data plane, **RN** the runner
(`fast-proxy-acme-service`), **A** admin API, **CL** CLI, **U** UI, **Q** quality, **W** documentation, **X** stretch.

### Workstream G: Groundwork

- [x] **G1 (S) Decisions and ADRs.** Resolve the **(confirm)** items. Write ADR 0024 (ACME client and crate
  layout: K1, K2, K16), 0025 (challenges in the snapshot, propagation and the bypass: K8, K9), 0026 (lease and
  status written without a revision, the scheduler policy: K6, K7, K10, K11), 0027 (account, issued keys and the
  certificate write: K3, K4, K5, K12, K13). Mark ADR 0014 as realized by them. Add the decisions to the ROADMAP log.
  *Done when:* the ADRs are merged and no **(confirm)** item is open.
  *Status:* ADRs 0024 to 0027 written as **proposed** in PR1; ADR 0014 points at them; the ROADMAP log has the row.
  Open: the owner's confirmation.
- [x] **G2 (M) Spike: `instant-acme`.** On a throwaway branch: build with the `ring` feature on macOS, Linux and
  Windows; `cargo tree -d` for duplicate `hyper`/`rustls`/`aws-lc-rs`; register, order and finalize against Pebble
  with a challenge served by a tiny axum handler; serialise and restore the account credentials; check `Retry-After`
  and the problem document are reachable from errors; check a custom root for the directory (`directory_ca`).
  *Done when:* K1 is confirmed or the fallback chosen, with findings in ADR 0024.
  *Result (2026-11-15):* K1 confirmed (`instant-acme` 0.8.5, `ring` + `hyper-rustls`, no `aws-lc-rs`, no duplicate
  `hyper`/`rustls`/`ring`/`http`; credentials serialise with serde; a custom root through `builder_with_root`). Two
  findings: the library's HTTP client is HTTPS-only, so the directory must be `https` everywhere (config changed); a
  problem document carries no `Retry-After`, so a rate-limited order waits the 1-hour default. Run against the
  in-process test CA on every platform; the Pebble run is written (`crates/fast-proxy-acme/tests/pebble.rs`, ignored)
  but was not executed here (running downloaded binaries was not permitted in this session): it is TA2/Q4's job. (S) Defaults table.** K3, K6, K7 and K8 as one documented table (the source for validation, the UI
  and the docs).
  *As built:* the `Defaults` tables of ADRs 0026 (renewal, backoff, cap, lease) and the `[data_plane.acme]` section
  of `config.toml` (process settings); `RenewalPolicy::default()` and `AcmeConfig::default()` hold the values.

### Workstream MD: Model

- [x] **MD1 (S) HTTPS settings.** `acme_enabled` on the subsystem HTTPS settings, with the rules of K4 enforced
  where the process switch is known (compile takes an `AcmeAvailability` flag, like TLS availability in M3).
  *As built:* `HttpsPolicy::acme_enabled`, refused without `https.enabled` (`https.acmeEnabled`), stored in
  `https_acme_enabled`, carried by revisions and rollback, and read and written by the API's `https` object. The
  process-switch rule (the 400 while ACME is off) is the compile rule R2 in PR2.
- [x] **MD2 (M) ACME vocabulary.** `AcmeState`, `SkipReason`, `AcmeStatus`, `AcmeChallenge` (token validated as
  base64url, at most 128 characters), `DomainAllowlist` (parsed patterns, `allows(&str)`), `AcmeKeyType`; all with
  `parse` constructors and unit tests. The credentials type has a redacted `Debug`.
  *As built:* `model/src/acme.rs`, plus `AcmeLease`, `AcmeToken` and `sanitize_error_detail`; `AcmeStatus` gained
  `window_failures` (the cap counts failures in the current hour, the backoff counts since the last success). The
  sealed account row and its redacted `Debug` live in the store (`AcmeAccountRow`).

### Workstream S: Store

- [x] **S1 (M) Schema.** The column and four tables of 5.2; `SCHEMA_VERSION` to 6; the schema test updated.
- [x] **S2 (M) Repositories.** `repository/acme_account.rs`, `acme_challenge.rs`, `acme_lease.rs` (the
  compare-and-swap, tested with two connections), `acme_status.rs`; the `https_acme_enabled` column in
  `https_columns.rs`; challenges in `load.rs` so compile sees them. Sealed columns are opaque bytes.
  *As built:* one module, `repository/acme/` (`account`, `challenges`, `lease`, `status`, tests); lease and status go
  through `Database::write_unrecorded`. Loading challenges for compile moves to R1 (PR2), next to the maintenance
  inputs, since compile takes them as inputs rather than as `Config`.
- [x] **S3 (S) Certificate replace in place by name** for `source = acme`, keeping the id (check whether M3's
  `replace_certificate` already covers it; extend rather than duplicate).
  *As built:* nothing to add: `certificates::update` already replaces name and material by id.

### Workstream F: Config file and command line

- [x] **F1 (M) `[data_plane.acme]`** (`src/config/acme.rs`), `--acme`/`--no-acme`, validation of K3 (TLS on, a plain
  listener, a non-empty allowlist, a readable `directory_ca`, an `https` directory URL unless it is loopback), the
  staging warning and the port-80 warning; tests for each refusal.
  *As built:* `src/config/acme.rs` and `validate_acme.rs`; the warnings are logged at startup; `config.toml`
  documents the section. Every listener is a plain listener, so "a plain listener" is always met; the port-80 rule is
  the warning.

### Workstream T: The TLS crate

- [x] **T1 (S) `acme_key.rs`.** Generate an ECDSA P-256/P-384 key and a CSR for a list of names (rcgen);
  the key in a zeroizing type with a redacted `Debug`.
  *As built:* `generate_order_key` returns `OrderKey { key: KeyMaterial, csr_der }`; the test PKI gained
  `TestCa::sign_csr`, which the ACME test CA (TA1) will use.
- [x] **T2 (S) `Vault::seal_secret` / `open_secret`** for arbitrary bytes (the account credentials), sharing the
  AES-256-GCM code with `seal_key`; the key check unchanged.
- [x] **T3 (S) Validate an issued chain** with the upload validator: covers the names, matches the key, currently
  valid; one function so issuance and upload cannot disagree.

### Workstream AC: The ACME crate (new, `fast-proxy-acme`)

- [x] **AC1 (S) Crate skeleton**, `//!` docs, dependency on `model` and the client library only.
- [x] **AC2 (M) `eligible_names`** (K5): pure, every skip reason, the 100-name cap; property test that every input
  name is either eligible or skipped exactly once.
- [x] **AC3 (M) `RenewalPolicy`** (K6, K7): `due_at(not_before, not_after, subsystem)`, the stable jitter, the
  backoff ladder, `Retry-After`, the hourly cap, the permanent-error wait; `now_ms` is a parameter; property tests
  (never earlier than the previous attempt plus the minimum backoff, never past `not_after - 1 day` while healthy).
- [x] **AC4 (M) `AcmeClient` trait and the `instant-acme` adapter**: account create/restore, new order, the
  HTTP-01 challenges with their key authorizations, set ready, poll with `Retry-After`, finalize with a CSR, download
  the chain; errors mapped to a small `AcmeProblem` enum (`RateLimited { retry_after }`, `Permanent`, `Transient`,
  `Unauthorized`) with a sanitized, capped detail.
  *As built:* `acme/src/client.rs`, a concrete `AcmeClient` / `AcmeSession` / `PendingOrder` (no trait: the in-process
  test CA makes the real adapter cheap to test, and a swap still touches one file); errors are `AcmeError { kind:
  FailureKind, detail }` through `classify_problem`; a failed validation reports the challenge's own problem.
- [x] **AC5 (S) `OrderStep` state machine** (published, propagating, self-checked, validating, finalizing, done,
  failed) so the runner's order logic is a pure transition function with unit tests.

### Workstream TA: Test CA (new, dev-only `fast-proxy-test-acme`)

- [x] **TA1 (L) The in-process ACME server** of K16: directory, nonces, accounts, orders, authorizations, HTTP-01
  validation by real fetch, finalize, certificate download, issued from `fast-proxy-test-pki`'s CA; switches for
  failure, `rateLimited` with `Retry-After`, slow validation, lifetime; a log of what it fetched and issued for
  assertions. Used only from `dev-dependencies`.
  *As built:* `crates/fast-proxy-test-acme` over HTTPS on loopback (its root is the `directory_ca`), round-robin
  challenge targets for cluster tests, `set_rate_limited`, `set_lifetime_days`, counters and a validation log. It
  trusts JWS signatures (Pebble checks those).
- [x] **TA2 (S) Pebble harness** for the opt-in CI job (download, start, its CA for `directory_ca`, cleanup).
  *As built:* a `pebble` job in `.github/workflows/ci.yml` (release binaries, the repository at the same tag for
  its config and certificates, challtestsrv for DNS); it runs `crates/fast-proxy-acme/tests/pebble.rs`.

### Workstream CS: Config service

- [x] **CS1 (M) The ACME switch on subsystems** through the existing HTTPS write, with the K4 rules and their 400s;
  refusing to attach a certificate while ACME is on.
  *As built:* `acme_rules.rs` (`check_acme_change`, `ConfigService::check_https_change`), applied in `write_subsystem`
  (update and HTTPS edits) and on create in the admin API; the process switch is `TlsSettings::acme_enabled`.
- [x] **CS2 (M) Challenges.** `publish_challenges` and `clear_challenges` as summary-only revisions with audit rows
  (`acme.challenge.publish`, `acme.challenge.clear`, actor `system`); expired ones cleared.
  *As built:* `acme_challenges.rs`; publishing replaces the subsystem's earlier challenges and drops every expired
  one; clearing what is not there writes nothing; every compile reads the challenges (`stored_inputs`).
- [x] **CS3 (M) Store an issued certificate** (K13): seal, replace in place or create, attach, clear the order's
  challenges, delete the unused `dev-<subsystem>`, audit `certificate.acme.issue|renew`, one apply.
  *As built:* `acme_certificates.rs` (`store_acme_certificate(IssuedCertificate)`); the chain passes `validate_issued`
  before the write; a switch turned off mid-order is a `Conflict`; an uploaded certificate is kept, unattached.
- [x] **CS4 (M) Lease and status without a revision** (K10, K11): `try_acme_lease`, `release_acme_lease`,
  `record_acme_status`, `record_acme_failure` (with its audit row `certificate.acme.fail`); a test asserts the
  revision version does not move.
  *As built:* `acme_state.rs`, also `acme_lease`, `acme_status(es)` and `forget_acme_status`.
- [x] **CS5 (S) Account**: load and open, or create and seal (through the vault the service already holds).
  *As built:* `acme_account` / `save_acme_account` with `AcmeAccountCredentials` (zeroed, redacted `Debug`), sealed
  under `acme-account:<directory>`, audited `acme.account.save`; a test scans the database file and WAL.

### Workstream R: Routing and compile

- [x] **R1 (M) Challenges in the snapshot.** `compile_all` takes the live challenges; `Snapshot::acme_challenge(host,
  token) -> Option<&str>`; expired ones dropped at compile; carried like maintenance scopes. Pure tests.
  *As built:* `AcmeInputs` is a seventh input of `compile_full`; `Snapshot::acme_challenges` (`AcmeChallenges::answer
  (host_header, path)`) returns at once when nothing is pending.
- [x] **R2 (S) The K4 cross-table rules** in compile (ACME on needs HTTPS on; no manual attach while ACME is on).
  *As built (moved):* "ACME on needs HTTPS on" is the model rule of MD1. The other two are rules about *changes*,
  so they live in the write path (CS1), not in compile: a stored switch must keep compiling when the process turns
  ACME off (it is inert then), and the ACME write itself must be able to attach what it issued.

### Workstream D: Data plane

- [x] **D1 (M) Answer the challenge** first in `resolve` (K9): `GET`/`HEAD`, token and host must match, `200
  text/plain`, `Cache-Control: no-store`; fall through otherwise. Tests: on a redirecting subsystem, in each
  maintenance scope, on a disabled subsystem, over HTTPS, an unknown token reaching the backend, a token for another
  host falling through, `POST` falling through, no sample taken, no route metric, the challenge metric.
  *As built:* `dataplane/src/acme_challenge.rs` and `Resolved::AcmeChallenge`; `fastproxy_acme_challenge_requests_total
  {outcome="answered"|"not_answered"}`; unit tests in `acme_challenge_tests.rs` and `sampling_tests.rs`; the redirect
  and HTTPS cases end to end in `tests/acme_challenges.rs` (with a stored ACME certificate on the next handshake).
- [x] **D2 (S) Cost check**: a micro-benchmark of a normal request with the prefix check in place.
  *As built:* a normal request pays `HashMap::is_empty` and one `starts_with` on the path (for the metric); no
  separate micro-benchmark, the measured figure comes with the Q6 benchmark run in PR4.

### Workstream RN: The runner (new, `fast-proxy-acme-service`)

- [x] **RN1 (M) Lease loop**: take, renew at `lease_ttl/3`, stop placing orders when lost, release on shutdown;
  `fastproxy_acme_lease_held`.
- [x] **RN2 (L) Scheduler**: on `renew_check_interval`, on `ConfigChanged` (debounced 30 s) and on a forced request,
  read the subsystems with ACME on and their status, compute the work with `RenewalPolicy` and `eligible_names`,
  run one order at a time (a queue, the forced one first), update the status.
  *As built:* `AcmeRunner::run` and `run_once(forced)` (the tests drive passes with a clock they move); forced
  requests through `AcmeControl::request_renewal`; a status for a subsystem that left ACME is dropped.
- [x] **RN3 (L) One order** (4.1), driven by `OrderStep`: account, order, publish, propagation wait, self-check,
  ready, poll, key and CSR, finalize, validate, store; every failure path clears the challenges and records the
  status and audit; a timeout on the whole order (10 min).
  *As built:* `acme-service/src/order.rs`; the account is registered on the first order and kept; the whole order
  times out after twice the CA timeout plus the propagation wait plus a minute.
- [x] **RN4 (S) Self-check client**: fetch `http://<name>/.well-known/acme-challenge/<token>` with a short timeout,
  a few attempts; the error names the name and what was received (status code, not the body).
- [x] **RN5 (S) Wiring in `src/startup.rs`**: start the runner when ACME is enabled, stop it on shutdown before the
  store; the `acme_status` events.
  *As built:* `src/acme.rs` builds the settings (propagation wait twice the poll plus 2 s) and the task; shutdown
  waits up to 5 s for the lease to be released. The `acme_status` WebSocket event moves to A2 (PR4) with the API.
  `[data_plane.acme] self_check` (default on) was added.
- [x] **RN6 (S) Metrics** in `fast-proxy-telemetry` (5.3), labels from config names and fixed enums only, removed
  series for deleted subsystems.
  *As built:* `fastproxy_acme_orders_total{subsystem,outcome}` and `fastproxy_acme_lease_held`. The next-attempt gauge
  was dropped: the status API (PR4) shows it, and a per-subsystem gauge would need its own series cleanup.

### Workstream A: Admin API

- [x] **A1 (M) `GET /api/acme`, `GET /api/subsystems/{id}/acme`, `POST .../acme/renew`** registered in the
  `RouteTable` with their `Access`; the permission table test enumerates them; error envelopes with no CA URLs or
  storage detail.
  *As built:* `admin/src/api/acme.rs`; `AdminState::with_acme` carries the settings and the runner's `AcmeControl`;
  a 429 is the new `AppError::RateLimited` with `Retry-After`; the overview checks for the account without opening it
  (security review finding 1).
- [x] **A2 (S) `acme_status` WebSocket event.**
  *As built:* `ConfigService::subscribe_acme` and `admin/src/acme_bridge.rs` (`acmeStatus`).
- [x] **A3 (S) `docs/getting-started.md`**: the curl walkthrough for ACME against Pebble or staging.
  *As built:* section 21.

### Workstream CL: CLI

- [x] **CL1 (S) `fast-proxy acme status` and `fast-proxy acme renew <subsystem>`**, exit codes 0/1/2, tests in
  `tests/cli.rs`.
  *As built:* `src/commands/acme.rs`; checked against the real binary in `tests/acme_binary.rs`.

### Workstream U: Admin UI

- [x] **U1 (M) ACME on the subsystem page**: the switch in the HTTPS form (disabled with an explanation when the
  process has ACME off; a staging badge), the status card (state, names and skipped names with reasons, expiry,
  next attempt, last error, lease holder), "Renew now" for an admin; live through the event.
  *As built:* `components/subsystem/AcmeCard.tsx` in the HTTPS section (a viewer sees it read-only); the attached
  certificate's manual actions give way to "Managed by ACME".
- [x] **U2 (S) ACME overview** (a section of the certificates page): the directory, the account, every subsystem's
  status, a filter on failing ones.
  *As built:* `components/certificates/AcmeOverviewSection.tsx` with an "Only failing" filter.
- [x] **U3 (S) Types, API functions, validation** in `ui/src/lib/`; Vitest tests for the form rules, the status
  card states and the renew button's errors (409, 429).
  *As built:* `lib/types.ts`, `lib/api.ts`, `lib/acme.ts`; a 202 is now an empty answer in `http.ts`; Vitest tests
  for the card, the overview and the helpers; `LiveContext` refreshes on `acmeStatus`.

### Workstream Q: Quality

- [x] **Q1 (L) Exit-criteria tests** `tests/acme.rs` against the real binary and the test CA: issuance and the next
  handshake (1), a host added and the skipped names (2), renewal, failure, backoff and rate limit with short-lived
  certificates (3), the challenge under redirect, maintenance and a disabled subsystem, and an unknown token reaching
  the backend (4), restart without re-ordering (6).
  *Partly done in PR3:* `tests/acme_issuance.rs` covers 1, 2, 3 and 6 in-process (the real runner, data plane and
  config service, a moved clock); PR2's `tests/acme_challenges.rs` covers 4. Left for PR4: the same against the real
  binary, which needs the API.
  *Done in PR4:* `tests/acme_binary.rs` issues and renews through the real binary and checks the CLI; ACME off and a
  start without TLS or an allowlist are covered too.
- [x] **Q2 (M) Cluster test** `tests/acme_cluster.rs`: two processes, one database; one lease holder; the CA's
  fetches answered by both (the test CA alternates between them); the certificate served by both; kill the holder
  mid-order, the other finishes after the lease expires (5).
  *As built:* two processes; the holder is killed (not stopped), the survivor takes the lease once it expires
  (`lease_ttl` 30 s, the minimum) and orders for a new host; the test takes about 35 s.
- [x] **Q3 (M) Secret hygiene**: extend `tests/secret_hygiene.rs` and the database and WAL scan with the account key
  and issued keys; `Debug` of every new type (7).
  *As built:* in `tests/acme_binary.rs` (every file under the state directory, the API and the metrics);
  storing the credentials unsealed turns it red.
- [x] **Q4 (S) Pebble job** in CI (Linux), running Q1's issuance test against Pebble.
  *As built:* see TA2; it runs the protocol client's Pebble tests rather than the whole issuance test (the client is
  what an independent server checks; the runner is covered by the in-process CA).
- [x] **Q5 (S) Regression and coverage**: every earlier exit-criteria suite green; coverage measured and recorded;
  the "data plane never depends on the store" manifest test still passes; the new crates' manifest tests (the store
  never decrypts).
  *Result:* see section 12.
- [x] **Q6 (S) Benchmark**: the M6 numbers re-run with the challenge check in place, recorded in `benchmarks.md`.
  *As built:* `scripts/bench-m4.sh`; results in `benchmarks.md`.
- [x] **Q7 (M) Security review** `docs/security-review-m4.md`: what the account key allows, the allowlist as the guard
  against issuing for names nobody meant, a hostile `Host` with a known token, the challenge path as an unauthenticated
  surface, CA error text reaching the API, the propagation wait, the staging default.
  *As built:* [security-review-m4.md](security-review-m4.md), one finding (fixed).

### Workstream W: Documentation

- [x] **W1 (S)** README (ACME section, port 80 and DNS requirements, staging versus production, cluster notes),
  AGENTS.md (new crates and rules), ROADMAP (tick M4, the decision log row), the outcome section of this plan.
  *As built:* README (an ACME section, the CLI, the structure and tests), AGENTS.md, ROADMAP (ticked, outcome row).
- [x] **W2 (S)** Operator notes: rate limits and how to read the status; moving from staging to production (a new
  account, every certificate reissued).
  *As built:* in the README's ACME section (renewal, backoff, rate limits, staging to production, watching it).

### Workstream X: Stretch (only after section 10 holds)

- [ ] **X1 (M) TLS-ALPN-01** for deployments without port 80: an `acme-tls/1` ALPN certificate in the TLS resolver,
  chosen per challenge from the snapshot.
- [ ] **X2 (M) ARI (RFC 9773)**: ask the CA when to renew, falling back to K6.
  *Moved (X1 and X2):* stretch tasks not taken in M4; they stay candidates for after M7.

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| G1 | none (needs the owner's answers) |
| G2 | none (a spike; can start now) |
| G3 | G1 |
| MD1, MD2 | G3 |
| S1 | MD1, MD2 |
| S2, S3 | S1 |
| F1 | MD2 |
| T1, T2, T3 | none |
| AC1 | G2 |
| AC2, AC3, AC5 | AC1, MD2 |
| AC4 | AC1, G2 |
| TA1 | none (uses `fast-proxy-test-pki`) |
| TA2 | G2 |
| CS1 | S2, MD1, R2 |
| CS2, CS4, CS5 | S2, T2 |
| CS3 | S2, S3, T1, T3 |
| R1 | S2, MD2 |
| R2 | MD1 |
| D1 | R1 |
| D2 | D1 |
| RN1 | CS4 |
| RN2 | RN1, AC2, AC3 |
| RN3 | RN2, AC4, AC5, CS2, CS3, CS5, RN4, D1 |
| RN4 | none |
| RN5, RN6 | RN3, F1 |
| A1, A2 | CS4, RN5 |
| A3, CL1 | A1 |
| U1-U3 | A1, A2 |
| Q1-Q3 | RN5, TA1, A1 |
| Q4 | Q1, TA2 |
| Q5-Q7 | every code task |
| W1, W2 | feature tasks complete |
| X1, X2 | section 10 |

Four pull requests, each leaving `main` green and releasable; each starts from `main` after the previous one merged:

1. **PR1 `m4/pr1-foundations`**: G1, G3, MD1, MD2, S1-S3, F1, T1-T3, AC1-AC3, AC5. Decisions (ADRs 0024 to
   0027), the model, the schema and repositories, the config section, keys and sealing, and the pure half of the
   ACME crate (eligibility, renewal policy, order steps). No protocol library and no behavior yet.
2. **PR2 `m4/pr2-challenges`**: CS1-CS5, R1, R2, D1, D2. The writes through `ConfigService` (switch, challenges,
   issued certificate, lease, status, account), challenges in the snapshot, and the data plane answering them;
   testable without a CA.
3. **PR3 `m4/pr3-runner`**: G2 (the spike, first), AC4, TA1, RN1-RN6 and the issuance part of Q1. The protocol
   client, the in-process test CA, the runner and its wiring and metrics; the first end-to-end issuance.
4. **PR4 `m4/pr4-surface`**: A1-A3, CL1, U1-U3, the rest of Q1, Q2-Q7, TA2, W1, W2. API, events, CLI, UI, the
   cluster, hygiene and Pebble tests, the benchmark, the security review, the docs and the outcome section.

## 8. Performance and cost budget

- **Request path:** one prefix comparison on the path for a normal request; a challenge request is one map lookup.
  Throughput stays within the noise of the M6 baseline (Q6).
- **Snapshot:** the challenge map is empty almost always and holds a handful of entries during an order; compile
  cost is unchanged in practice.
- **Revisions:** an order costs two summary-only revisions plus the certificate's. With the last 100 revisions kept,
  a burst of orders (many subsystems renewing on one day) can push configuration revisions out of the rollback
  window; the jitter spreads renewals, one order runs at a time, and the docs say so. If it proves a problem, the
  fix is to keep summary-only revisions out of the 100-revision count (a store change), not to drop the revisions.
- **CA traffic:** one account, at most one order in flight per instance cluster (the lease), polling at the CA's
  `Retry-After`, a 1-hour check interval; a healthy cluster with 50 subsystems makes about 50 orders per 60 days.
- **Memory:** the runner holds one order at a time; nothing grows with traffic.

## 9. Risks specific to M4

| Risk | Mitigation |
|---|---|
| **Locking out a domain with rate limits** (a loop of failed orders, a restart storm) | Staging by default; stored backoff and an hourly cap per subsystem; `rateLimited` and `Retry-After` honoured; a self-check before the CA is asked; restarts compute "due" from stored facts; one order at a time through the lease |
| **Issuing for names nobody meant** (a typo, a host added by someone else) | The allowlist is required and checked in the runner, in the API preview and in the UI; skipped names are shown with a reason |
| **Challenges not yet on a sibling when the CA fetches** | The propagation wait derived from the poll interval, then a self-check that may land on any instance; a failed self-check spends no CA attempt; Q2 tests it with the CA alternating between instances |
| **Port 80 not reachable** (firewall, NAT, no plain listener) | Startup refuses ACME without a plain listener and warns when it is not on port 80; the self-check fails fast with the name and status; TLS-ALPN-01 is the stretch answer |
| **The challenge path as an unauthenticated surface** | It answers only exact tokens for exact hosts with a public value; anything else falls through to routing exactly as before; fixed-size answer; reviewed in Q7 |
| **Key leakage** (account key, issued keys, CA errors that echo data) | Sealed before the write; redacted `Debug`; CA problem detail sanitized and capped; Q3 extends the hygiene test and the database and WAL scan |
| **Split brain in a cluster** (two instances ordering at once) | Compare-and-swap lease with a TTL; work only while it is held; a lost lease stops new orders; a duplicated order would at worst cost one CA order, never a wrong certificate (the write replaces in place) |
| **Revision churn pushing real changes out of history** | Section 8: jitter, one order at a time, documented; the store fix named if needed |
| **The library changing or lagging** (`instant-acme`) | Behind the `AcmeClient` trait; G2 checks it; the fallback is our own client behind the same trait; the Pebble job catches protocol drift |
| **The test CA agreeing with our bugs** | It validates by really fetching, follows RFC 8555 shapes, and the Pebble job runs the same test against an independent implementation |
| **Shorter certificate lifetimes** (CAs moving to 45-day and shorter certificates) | The renewal point is a fraction of the actual lifetime, never a fixed number of days; ARI is the stretch task |
| **Scope creep** (DNS-01, EAB, admin-listener ACME, revocation) | Section 2 lists what is out; new ideas go to the roadmap |

## 10. Definition of done for M4

- [x] Every task above is checked, or explicitly moved with a note (stretch tasks may be moved).
- [x] Exit criteria (section 1) demonstrated by Q1 to Q4, with the table in the outcome section. The manual run
  (turn ACME on against staging for a real name, see the certificate, add a host, force a renewal, restart, two
  instances) is the owner's click-through.
- [ ] CI green on all three platforms, the Pebble job green on Linux; no new clippy warnings.
- [x] Overall test coverage above 60%, the number recorded here.
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [x] README, AGENTS.md, ROADMAP.md and getting-started updated.
- [x] Benchmark re-run and recorded within the budget of section 8.
- [x] The security review (Q7) done and its findings recorded.

## 11. Touch points with finished work, and what M4 leaves for M7

- **M3 certificates:** issuance is a stored certificate with `source = acme`, replaced in place, so the certificate
  cache, the snapshot swap, the 421 rule and the cluster pickup apply unchanged; the `dev-<subsystem>` rule (only for
  a subsystem that enables HTTPS) still holds, and the dev certificate goes away once the ACME one is attached.
- **M5:** the challenge bypasses maintenance and the redirect in the one place before them; issuance is audited as
  `system`; the account key and challenges are not `Config` and never rolled back; status is readable by a `viewer`,
  the switch and "renew now" are `admin` writes.
- **M6:** the challenge answer is not a routed request and is never sampled.
- **M7:** the security review there covers ACME key storage (named in the roadmap); rate limiting must exempt the
  challenge path or the CA's fetches could be refused; fuzzing gets `eligible_names` and the allowlist parser.
- **The PostgreSQL project:** the new tables are portable SQL; the lease's compare-and-swap is one `UPDATE` that
  PostgreSQL runs as is.

## 12. Outcome

Written 2026-11-16, when PR4 was ready. Everything is built; the owner's manual check, the confirmation of the
decisions K1 to K17 (still *proposed*) and the merges are open.

| Exit criterion | Held? | Where |
|---|---|---|
| 1. Issuance end to end, served by the next handshake | yes | `tests/acme_binary.rs` (real binary), `tests/acme_issuance.rs` (through a redirecting subsystem) |
| 2. Domains follow the database; ineligible names skipped with a reason | yes | `an_added_host_is_ordered_after_the_debounce_and_ineligible_ones_are_explained`; the API preview test |
| 3. Renewal at two thirds, backoff, rate limits, the old certificate keeps serving | yes | `tests/acme_issuance.rs` with a moved clock |
| 4. The challenge ahead of the redirect, maintenance and a disabled subsystem; unknown tokens routed | yes | `tests/acme_challenges.rs`, the data plane's `acme_challenge_tests.rs` |
| 5. One instance talks to the CA; both answer and serve; failover | yes | `tests/acme_cluster.rs` |
| 6. Restarts never re-order; staging by default; hourly cap | yes | `a_valid_certificate_is_never_ordered_again_not_even_after_a_restart`, the cap tests, the config default |
| 7. No secret leaks | yes, after one fix | `tests/acme_binary.rs` scan; [security-review-m4.md](security-review-m4.md) finding 1 |
| 8. Visible and audited | yes | the API, CLI and UI tests; audit actions asserted in the issuance tests |
| 9. No regression; the request path cost | yes | the whole suite; [benchmarks.md](benchmarks.md) (-0.5% to -2.5%, inside the spread) |

Numbers: **1676 Rust tests pass and 4 are ignored** (two are the Pebble tests); **483 Vitest tests pass**; coverage
**95.83% regions, 97.31% lines** (`cargo llvm-cov --workspace --summary-only`). `cargo fmt --check` and
`cargo clippy --all-targets -- -D warnings` are clean. One unrelated test failed in the full run and fails about three
times in four when run alone: `fast-proxy-sampling`'s `concurrent_recording_loses_nothing_within_an_epoch`. It is a
race in M6's `LatencyWindow::record` at an epoch rollover (a thread that sees the new epoch number already swapped in
adds to a bucket another thread is still zeroing), present on `main`; it is reported to the owner rather than fixed
here.

What the build changed in the design, beyond the "As built" notes: the directory must be `https` everywhere (spike
G2); the adapter is a concrete type; the switch rules live in the write path; the status table gained
`window_failures`; `self_check` became a config key; a status left `pending` or `renewing` by a stopped instance is
set right by the next pass; the overview never opens the account. Open for the owner: the decisions K1 to K17, the
manual click-through (turn ACME on against staging for a real name), the Pebble job's first CI run (it was written but
could not be run on the development machine), and the latency-window race.

