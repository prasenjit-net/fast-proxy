# Security review of M4 (ACME automatic certificates)

Task Q7 of [M4-PLAN.md](M4-PLAN.md): the checklist run before the release, with how each item
was checked and what was found. Date: 2026-11-16. Scope: everything M4 added: the
`[data_plane.acme]` settings, the account and issued keys, the challenge path in the data
plane, the lease, status and account rows, the runner and its self-check, the protocol client,
the ACME API, CLI and UI. The M3, M5 and M6 reviews still hold for certificates, keys, tokens,
the admin plane and sampling.

M4 adds three things worth an attacker's interest: **a new secret** (the ACME account key,
which can order certificates for every name the account has validated), **a new
unauthenticated surface** (`/.well-known/acme-challenge/` on every listener) and **a new
outbound client** (to the CA, and the self-check to the proxy's own names).

## What is new at rest

| Item | Where | How it is protected |
|---|---|---|
| Account credentials (account URL and private key) | `acme_account.credentials_enc` | sealed by the vault (AES-256-GCM, label `acme-account:<directory>`); opened only to restore the session for an order |
| Issued private keys | `certificate.key_enc` (`source = acme`) | sealed like every certificate key; a fresh key per order |
| Challenges (token, key authorization, names) | `acme_challenge`, the snapshot | public by design (the CA fetches them over plain HTTP); expire after an hour |
| Lease, status | `acme_lease`, `acme_status` | no secret; the last error is sanitized (one line, 300 characters) |

## Checklist

| # | Item | Result | How it was checked |
|---|---|---|---|
| 1 | No private key or account credential is in any file the proxy wrote, any API answer or metric | pass | `tests/acme_binary.rs` issues and renews through the real binary, then scans every file under the state directory (the database, its WAL and SHM) for `PRIVATE KEY` and `key_pkcs8` (the field name of the serialized credentials) and reads `/api/acme`, `/api/certificates`, the audit log and `/metrics`. Storing the credentials unsealed turned it red (the WAL held `key_pkcs8`). The config service test scans the database for the credentials' bytes |
| 2 | A status read never opens a secret | pass, after one fix (finding 1) | `GET /api/acme` reports whether an account exists from the row alone (`acme_account_registered`) |
| 3 | Every `Debug` of a type holding a secret is redacted | pass | `AcmeAccountCredentials`, `AcmeAccountRow`, `OrderKey`, `IssuedCertificate`, `RunnerSettings` (no clock, no email); unit tests for each that print them |
| 4 | Names are certified only inside `allowed_domains` | pass | `eligible_names` is the one rule (property test: every host is eligible or skipped exactly once, every eligible name is allowed); the allowlist is required when ACME is on and refused empty; the API preview and the UI show the skipped names with reasons |
| 5 | The challenge path answers nothing but a live token for its own host | pass | `GET`/`HEAD` only; the token and the `Host` must match a stored challenge; anything else is routed as before, so the path adds no new behavior for unknown tokens. The answer is the key authorization (at most 193 characters, checked when the challenge is built) with `Cache-Control: no-store`. Data plane tests cover another host, another method and an unknown token |
| 6 | The challenge path cannot be used to bypass maintenance or the redirect for anything else | pass | It is checked first in `resolve` and answers itself; it never forwards. Tests under every maintenance scope and on a redirecting subsystem show the answer and a 503 or 308 for any other path |
| 7 | Challenge requests carry no request value into metrics | pass | `fastproxy_acme_challenge_requests_total{outcome}` with two fixed outcomes; a unit test checks a token never becomes a label |
| 8 | The CA's text cannot inject into logs, the API or the UI | pass | Problem details go through `sanitize_error_detail` (one line, control characters removed, 300 characters); the UI renders them as text |
| 9 | The CA is reached over authenticated TLS only | pass | The directory must be `https` (config validation); the client trusts the platform roots, or only the `directory_ca` file when one is given |
| 10 | A loop of failures cannot spend the CA's rate limits | pass | Stored backoff (1 min to 6 h), a 6 h wait for a refusal that cannot succeed, `Retry-After` honoured, at most 5 failed orders per subsystem per hour even when forced (property test); a failed self-check costs no CA validation; the default directory is Let's Encrypt staging |
| 11 | Only one instance orders at a time | pass | The compare-and-swap lease (a test with four connections racing on one file); `tests/acme_cluster.rs` with two processes: one holder, failover after the holder dies |
| 12 | A forced renewal is an admin write and audited | pass | `POST /api/subsystems/{id}/acme/renew` needs `Write` (a viewer gets 403), is refused while an order runs (409) or the cap is spent (429), and writes `acme.renew.request`. Issuance, renewal and failure are audited as `system` |
| 13 | ACME state is not configuration | pass | Account, lease, status and challenges are not in `Config` or a revision's configuration copy; a rollback cannot restore a challenge or an old account. Challenges and certificates are summary-only revisions (so other instances follow); lease and status writes create no revision (tests assert the version does not move) |
| 14 | An issued chain is checked before it is stored | pass | `validate_issued`: it must match the order's key, be current and cover every ordered name; a chain missing a name stores nothing (config service test) |
| 15 | Turning ACME off keeps serving | pass | The switch can be turned off at any time; the last ACME certificate stays attached; a subsystem whose stored switch is on keeps compiling when the process has ACME off |

## Findings

### 1. `GET /api/acme` opened the account to say whether one existed (fixed)

The overview reported `accountRegistered` by calling `acme_account`, which opens the sealed
credentials with the vault. A read by any viewer therefore decrypted the account key into
memory, and a database with a damaged or foreign account row made the overview fail with a
409. Found by the hygiene test of item 1, when an unsealed row made the overview fail instead
of the scan. Fixed: `ConfigService::acme_account_registered` checks for the row without opening
it; only the runner opens the account, to order.

## Known limits

- **The account key lives in memory** while the runner holds a session, inside the protocol
  library's types, which are not zeroized on drop. The sealed copy at rest is unaffected.
- **The account email is stored in clear** (it is a contact the CA publishes in its own
  records, not a secret).
- **The self-check connects to the subsystem's own names on port 80.** An administrator who can
  add hosts can already point upstreams anywhere, so this adds no reach; behind a NAT without
  hairpin routing it fails every time, and `self_check = false` turns it off.
- **The Pebble job** checks real JWS signing and the protocol against an independent server; the
  in-process test CA trusts signatures. The job runs in CI (Linux); it was not run on the
  machine this review was written on.

## Residual risk

Anyone with the database **and** the master key can order certificates for the names the
account has validated, as with every sealed key since M3: keep the master key out of the
database's backups. Anyone who can write `config.toml` can point ACME at another CA or widen
the allowlist; the file is process configuration and is trusted.
