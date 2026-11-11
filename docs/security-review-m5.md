# Security review of M5 (admin authentication, audit, maintenance, CLI)

Task Q8 of [M5-PLAN.md](M5-PLAN.md): the checklist run before the release, with how
each item was checked and what was found. Date: 2026-11-11. Scope: everything M5 added
(tokens and roles, the guard, admin TLS and the exposure rules, the audit log, maintenance
mode and its bypass, the CLI, and the UI's handling of the token). The M3 review
([security-review-m3.md](security-review-m3.md)) still holds for certificates and keys;
its main residual (the admin plane having no authentication) is what this milestone
closes.

## Checklist

| # | Item | Result | How it was checked |
|---|---|---|---|
| 1 | Tokens and bypass secrets are stored only as hashes and shown once | pass | The token and maintenance tables hold `SHA-256(secret)` only (`SecretHash`, no plaintext column). `tests/secret_hygiene.rs` creates tokens, sets a generated and a chosen bypass secret, uploads a certificate, then scans every file under the state directory (the database and its write-ahead log included) for each secret, the key body and `PRIVATE KEY`. A planted leak (the token secret put in an audit target) turned it red |
| 2 | Comparison is constant time | pass | `secret_matches` hashes the presented value and compares the two digests with `subtle::ConstantTimeEq`; the bypass header uses the same function and only when the header is present. A token is looked up by its numeric id, never by comparing secrets, so the comparison is between fixed-size digests |
| 3 | No secret appears in any output | pass, after one fix (finding 1) | `secret_hygiene.rs` reads every API list and detail, every revision, the audit log, `/metrics`, the WebSocket frames, the console and the access log. `PlaintextToken`, `GeneratedSecret` and `SecretHash` have redacted `Debug`. The bootstrap token is printed once, on the console only (the same test counts it) |
| 4 | The throttle and the `Host` check are in force | pass | `tests/operations.rs` sends twelve wrong tokens to the real binary: ten are 401, then 429, and the right token is refused too. `tests/admin_auth.rs` sends a foreign `Host` over a real socket (421). The guard's unit tests cover the window, the address cap and that successes never count |
| 5 | The CSP is real | pass | `ui_page_policy_tests` fetches the built page the way a browser would and checks it against the header the server sends: no inline script, handler or `javascript:` URL, nothing from another origin, every asset served with a type `nosniff` allows. Planting an inline script, a foreign script and an `onload` handler each turned it red. `style-src` still allows inline styles (React sets style attributes); that cannot run script |
| 6 | Keys need an admin token over a protected channel | pass | `tls_api_tests` in the admin crate: a key upload is refused on an unprotected channel, accepted over admin TLS from any peer, refused for a viewer; a rename does not need the channel |
| 7 | An unprotected admin plane cannot be exposed | pass | `tests/admin_auth.rs` starts the real binary with a non-loopback admin listener and no TLS: startup fails naming the key to set. `auth = "off"` on a non-loopback listener is refused the same way |
| 8 | Audit entries are free of secrets and written for denials | pass | `tests/operations.rs` makes a viewer's write (403), a wrong token (401) and an operator's changes, and reads the trail back: the actor, role, source and revision are right, a denial is `access.denied` with the viewer's name, a bad token is `auth.failed` as `anonymous`. The same test scans the trail for every token secret |
| 9 | The maintenance bypass cannot be forged through headers | pass, after one fix (finding 1) | `operations.rs` and the data plane's tests: a claimed `X-Forwarded-For` from an untrusted peer does not bypass; a wrong secret, the right secret in another header and a header on a scope that has none do not bypass; only the right header or a listed network does. A trusted proxy's forwarded address is read from the right |
| 10 | The custom body and the message cannot be injected into the page | pass, with a note | The message is HTML-escaped in the built-in page and escaped as a JSON string in the JSON form (tests with markup and control characters). The operator's own body is served as written with the content type they chose, now with `nosniff` (finding 2). An administrator can therefore serve any HTML on a proxied host through it, which is the point of the feature and no more than they can do with a backend |
| 11 | The last admin token cannot be revoked | pass | `operations.rs` revokes a spare admin, then is refused (409) for the last one and still authenticates. Removing the guard turned it red |
| 12 | The bootstrap token is printed once | pass | `admin_auth.rs`: a second start prints nothing and the first token still works; `secret_hygiene.rs`: it appears on the console at most once, in no file, log line, response or event. A seeded token (`FAST_PROXY_BOOTSTRAP_TOKEN`) must be at least 32 characters |
| 13 | The WebSocket ticket is single-use and stays out of logs | pass | Tickets live 30 seconds in a bounded in-memory store and are removed when used; the access log keeps only `/ws` (a test removes the redaction and goes red); `secret_hygiene.rs` checks the logged address |
| 14 | Maintenance never reaches an upstream and leaves no endpoint state | pass | `maintenance_under_load.rs` toggles every scope under traffic and compares the backend's request count with the number of 200s (equal), and checks the sharp rule after each toggle |
| 15 | The guard covers every route | pass | `security_tests` builds the list from the router: every route declares an `Access`, every protected route refuses a missing token with 401, and a viewer is refused every non-read |

## Findings

### 1. The bypass secret was forwarded to the upstream (fixed)

A request let through by the secret header was forwarded with that header intact, so the
backend, and anything that logs its requests, received the bypass secret. Found while
checking item 9 by reading what `evaluate` leaves on the request. Fixed: the pipeline now
removes the bypass header of every scope that applied from the request before it is
forwarded, whether or not the secret matched (a wrong guess is not passed on either).
`the_bypass_secret_never_reaches_the_upstream` sends the right secret and a wrong one to a
real upstream and checks the headers it received; it failed before the fix.

### 2. The maintenance answer had no `nosniff` (fixed)

The 503 sent its content type but not `X-Content-Type-Options: nosniff`, so an operator's
`text/plain` body could be sniffed as HTML by an old browser. Fixed: the header is on every
maintenance answer; a data plane test asserts it.

### 3. Authentication results and audit volume were not metrics (fixed)

Exit criterion 9 asks for them and no task built them. Added `fastproxy_admin_requests_total
{result}` (`ok`, `unauthenticated`, `forbidden`, `throttled`, `host_refused`) and
`fastproxy_audit_entries_total{outcome}`, with labels from a fixed set.

## Residual risks (known and accepted)

- **`--token` shows in the process list.** Use `--token-file` (the CLI refuses a file readable
  by others on Unix) or `FAST_PROXY_TOKEN` (visible in the process environment to the same
  user and root). The CLI help and README say so.
- **The token lives in `sessionStorage`.** Script running in the admin origin could read it.
  The CSP (`script-src 'self'`, no inline script, no third-party origin) is what makes that
  acceptable; the sign-in page says so, and **Sign out** forgets it.
- **The throttle is per source address and per instance.** Behind a reverse proxy every
  request has the proxy's address, so one client's failures can throttle everyone, and with
  several instances the allowance multiplies. Real rate limiting is M7. The README already
  says a reverse proxy in front needs its own access control.
- **Entries for unauthenticated requests are capped at 60 a minute** for the process, so a
  flood cannot grow the log; failures beyond the cap are not individually recorded (the
  throttle still acts, and `fastproxy_admin_requests_total{result="unauthenticated"}` counts
  every one). An entry's target is the request path, bounded by the HTTP parser's header
  limit.
- **Anyone who can write the database file or run the offline `token` commands can mint a
  token.** The offline commands are the documented recovery path and need the same access as
  the file; protect the state directory.
- **Maintenance expiry depends on clocks.** The expiry is an absolute time; instances with
  skewed clocks end it at different moments (by the skew). NTP is assumed and documented.
- **A bypassed client learns which scope it passed** (`X-Maintenance-Bypass: global`). Only a
  client that already passed a bypass sees it.
- **Tokens never expire unless asked.** The UI offers expiry choices and the CLI an
  `--expires-days` flag; the default is no expiry, as for most API tokens.

## Result

Two real defects were found by reading the code against the checklist (the bypass secret
reaching the upstream, and the missing `nosniff`) and one gap against the exit criteria
(the two metrics); all three are fixed with tests that failed first. The rest of the
checklist held, and each mutation tried against the end-to-end tests turned them red. No
open issue blocks the release.
