# ADR 0013: upstream TLS

Status: accepted. Plan reference: M3 F3, F15, F16, F17.

## Context

Backends increasingly require TLS, often with internal CAs or self-signed
certificates and sometimes addressed by IP.

## Decision

- **Per group, in the database:** `tls` (off by default), `verify` (on by
  default), `server_name` (override for SNI and the certificate check; empty
  means the endpoint's host) and `ca` (the name of an uploaded CA bundle; empty
  means the OS trust store via `rustls-native-certs`).
- CA bundles are public material in their own table, referenced by name. The
  compile veto rejects an unknown name, also on rollback.
- `verify = false` is allowed but logged as a warning on every load that
  contains it, shown with a badge in the UI and exposed as a label on the
  upstream-TLS metric.
- **Health checks:** an HTTP probe to a TLS group uses TLS with the group's
  settings, so it tests the real path; the TCP probe stays a plain connect. No
  new health kind.
- Upstream connections are HTTP/1.1 over TLS; pools are kept per
  `(connect timeout, TLS settings)`; retries, passive health and body replay are
  unchanged.
- The upstream URI scheme follows the group's `tls` flag (`build_upstream_uri`).

## Consequences

- Tests always upload an explicit CA; the OS store is only the user default.

## Alternatives considered

- Global upstream trust settings: groups differ.
- A TLS health kind: the HTTP probe over the group's TLS already covers it.
