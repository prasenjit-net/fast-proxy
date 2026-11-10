# ADR 0016: admin TLS and exposure rules

Status: proposed (awaiting owner confirmation); spike G2 will add its findings
here. Plan reference: M5 H4, G2.

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
- **Spike G2** proves before building: the Axum router served through the
  existing TLS stack keeps `ConnectInfo`, a WebSocket upgrades through it, and the
  `Host` check and constant-time comparison work in the same middleware stack.

## Defaults

| Setting | Default |
|---|---|
| `[admin.tls] enabled` | `false` (required when the listener is not loopback) |
| `[admin.tls] names` | `["localhost"]` |
| `[admin.tls] certificate` | `admin` |
| `[admin] allow_insecure_remote` | `false` |

## Consequences

- A generated certificate means browser warnings until an uploaded one replaces it;
  the Certificates page says so.
- The admin certificate may be ACME-issued later (M4) with no change here.
