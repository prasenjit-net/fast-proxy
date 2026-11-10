# ADR 0016: admin TLS and exposure rules

Status: proposed (awaiting owner confirmation); built in M5 PR5 and the spike
findings are below. Plan reference: M5 H4, G2.

## Context

A bearer token must not cross a network in clear text, and the admin plane must
not be reachable unprotected by accident. Certificates already live in the
database (ADR 0010), so the admin listener should use the same machinery.

## Decision

- **`[admin.tls] enabled`** makes the admin listener serve HTTPS. The certificate
  is a stored one chosen by name (`certificate = "admin"`); if absent, a
  self-signed `admin` certificate is generated for `[admin.tls] names` through the
  normal write path (`ConfigService`), and an uploaded certificate of that name
  replaces it live. The plain listener is never opened when TLS is on.
- **Exposure rule.** Binding `admin.listen` to a non-loopback address requires
  both TLS and `auth = "required"`; otherwise startup fails with a message naming
  the key to set. `allow_insecure_remote = true` is the explicit, warned override
  for the TLS requirement. `auth = "off"` on a non-loopback address is refused
  even then.
- **Key-bearing requests** (uploading a private key) require an `admin` token and
  a **protected channel**: loopback, or admin TLS on. This supersedes the M3
  loopback-only rule (F27); its tests move to the new rule. A reverse proxy in
  front makes every request look local, so the README says it needs its own access
  control.
- **Spike G2** was folded into the build (AT1); its findings are under
  "Findings" below.

## Defaults

| Setting | Default |
|---|---|
| `[admin.tls] enabled` | `false` (required when the listener is not loopback) |
| `[admin.tls] names` | `["localhost"]` |
| `[admin.tls] certificate` | `admin` |
| `[admin] allow_insecure_remote` | `false` |

## Findings (spike G2, confirmed by the AT1 tests)

- The router is served by a small accept loop in the admin crate: a
  `tokio_rustls` handshake under a 10 s timeout and a bound of 64 concurrent
  handshakes, then hyper-util's auto builder with `serve_connection_with_upgrades`
  (so HTTP/1.1, HTTP/2 and the WebSocket upgrade all work) and
  `GracefulShutdown` (open connections get 10 s to finish).
- `ConnectInfo<SocketAddr>` does **not** come for free on a custom accept loop:
  the loop adds it to every request with `map_request`. Without it the
  access-log middleware refuses the request, so the omission cannot go unnoticed.
- The certificate resolver reads the live certificate cache **by name on every
  handshake** (`ConfigService::certificate_by_name`), so an uploaded or renewed
  `admin` certificate is served by the next connection with no restart. No
  per-connection certificate pinning is needed here: the admin plane has no 421
  rule.
- The `Host` check, the constant-time comparison and the whole guard stack run
  unchanged under TLS: they are ordinary middleware over the same router.
- Deleting or renaming the certificate the admin plane serves is refused with a
  409 (the listener finds it by name); replacing its certificate and key, which is
  how it is renewed, is allowed.
- An existing certificate of that name is never regenerated: the generated one is
  created only when none exists, so a restart serves the same certificate.

## Consequences

- A generated certificate means browser warnings until an uploaded one replaces it;
  the Certificates page says so.
- The admin certificate may be ACME-issued later (M4) with no change here.
