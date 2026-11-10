# ADR 0012: redirect, HSTS and the 421 rules

Status: accepted. Plan reference: M3 F13, F14, F18.

## Context

Subsystems opt into HTTPS; plain HTTP should be able to redirect, browsers
should be able to pin HTTPS, and a request must not be served by a connection
whose certificate was not meant for its host.

## Decision

- **Redirect and HSTS are per subsystem, under `https`.** `redirect` is `off`,
  `308` (default when on) or `301`; HSTS has `enabled`, `max_age_secs` (0 to two
  years) and `include_subdomains` (no `preload` in M3). Both are accepted only
  when `https.enabled` is true (a 400 names the field) and are effective only
  while TLS is on for the process.
- **Redirect** applies to requests that arrived on a plain-HTTP listener, after
  the host matched a subsystem and before routing. The target is
  `https://<matched request host>[:public_port]<path and query>`; the port is
  the HTTPS listener's `public_port` and is omitted at 443. The `Location` is
  built only from a host that matched a configured pattern. The check lives in
  one function so ACME HTTP-01 (M4) can bypass it.
- **HSTS** is sent only on HTTPS responses (RFC 6797).
- **421 on the HTTPS listener:** a host of a subsystem with HTTPS disabled; a
  host served by a different certificate than the one SNI selected (hosts that
  share one certificate pass, keeping HTTP/2 connection coalescing working); no
  SNI skips the second rule.
- **Scheme awareness (F18).** The connection context carries scheme, TLS
  version, ALPN, SNI and certificate id; `X-Forwarded-Proto` and `Forwarded` use
  the real scheme, client-supplied values still untrusted unless the peer is in
  `trusted_proxies`.

## Consequences

- A redirect to a listener that does not exist cannot happen (TLS off means no
  redirect).
- A wrong HSTS setting is hard to undo, so the UI confirms it.

## Alternatives considered

- Comparing names instead of certificates for 421: breaks coalescing.
- Process-wide redirect: subsystems differ in readiness.
