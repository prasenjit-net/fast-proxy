# ADR 0015: admin authentication, roles, bootstrap and recovery

Status: proposed (awaiting owner confirmation). Plan reference: M5 H1, H2, H3, H5,
H6, H18.

## Context

The admin plane trusts whoever can reach it. M5 must make it safe to expose
beyond loopback, without accounts, sessions or anything to store but hashes, and
without a way to lock the owner out.

## Decision

- **Bearer tokens**: `Authorization: Bearer fp_<id>.<secret>`. A token has a
  unique name (1 to 63 characters), a role and an optional expiry. No passwords,
  cookies or sessions, so there is nothing for CSRF to ride on.
- **Two roles.** `admin` can do everything. `viewer` can read everything except
  key material, and cannot write, manage tokens or export the audit log. The
  permission table is one exhaustive function; every admin route declares the
  `Action` it needs (`read`, `write`, `keys`, `tokens`, `audit-export`) and a
  test fails for a route without one.
- **Storage.** The secret is 256 random bits, shown once at creation. The database
  keeps the id and `SHA-256(secret)`, compared in constant time. A fast hash is
  right for a high-entropy random secret: there is nothing to brute-force.
- **No lock-out.** The last usable admin token cannot be revoked, expired or
  demoted (409).
- **Bootstrap.** With no admin token in the database the first start generates
  one named `bootstrap`, prints it once, and logs only the marker
  `admin token created`. `FAST_PROXY_BOOTSTRAP_TOKEN` seeds it instead (used only
  while no admin token exists).
- **Recovery.** `fast-proxy token create --role admin` works directly on the
  database file for anyone who can read the state directory, who could anyway.
- **On by default.** `[admin] auth = "required"`. `auth = "off"` is accepted only
  while the listener is bound to loopback and is warned on every start. There is
  no "open while no token exists" mode.
- **WebSocket.** `POST /api/ws-ticket` returns a single-use ticket valid for 30
  seconds, kept in memory on the issuing instance; `/ws?ticket=` consumes it and
  the socket carries its role. The access log redacts the `/ws` query.
- **Browser and brute-force protection.** A `Host` allow-list (listen address,
  `localhost`, `127.0.0.1`, `[::1]`, plus `allowed_hosts`) answers 421 otherwise,
  which stops DNS rebinding. Responses carry a self-only CSP,
  `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
  `Referrer-Policy: no-referrer` and `Cache-Control: no-store` on the API. After 10
  failed authentications from one address within a minute that address gets 429
  with `Retry-After` for a minute, even for a valid token (instance-local; real
  rate limiting is M7).
- **The UI** keeps the token in `sessionStorage`, sends it as a header, returns to
  sign-in on any 401, and shows a `viewer` a read-only UI. The CSP forbids inline
  script and third-party origins, which is what makes `sessionStorage` acceptable.

## Defaults

| Setting | Default |
|---|---|
| `[admin] auth` | `required` |
| `[admin] allowed_hosts` | empty (listen address and loopback names are implicit) |
| Token secret | 256 bits, base64url |
| Ticket lifetime / uses | 30 s / one |
| Failure throttle | 10 failures per minute per address, then 429 for a minute |

## Consequences

- Tests and the UI dev workflow move to authentication (M5 Q1): `auth = "off"` on
  loopback in the binary tests, a token helper in the HTTP-level tests.
- A token in a browser is exposed to an XSS bug; the CSP and a short-lived
  `sessionStorage` limit that, and the sign-in page says so.

## Alternatives considered

- **Accounts with passwords or SSO:** more machinery than a single-binary ingress
  needs; revisit after 1.0.
- **Cookie sessions:** need CSRF protection and server-side state.
- **Open until the first token exists:** an unprotected window on every fresh
  install; the bootstrap token makes the secure default as easy.
