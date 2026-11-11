# ADR 0018: maintenance mode

Status: proposed (awaiting owner confirmation); built in M5 PR6. Plan reference: M5 H9 to H14.

## Context

Disabling a subsystem answers a bare 503 with no explanation, no expiry and no way
to verify a fix before reopening. Operators need a temporary, reversible state
that explains itself and ends by itself.

## Decision

- **Three scopes**: global, subsystem, route, in a `maintenance` table (cascade on
  deleting the subsystem or route). Each has a message, an optional body and
  content type (at most 16 KiB; `text/html`, `text/plain` or `application/json`),
  `retry_after_secs` (1 to 86,400; default derived from the TTL, else 120), bypass
  rules, an optional expiry, and who set it and when. A request must be allowed by
  **every** active scope that applies; bypass rules are per scope.
- **TTL as an absolute `expires_at`** (wall clock, ms). The data plane checks it at
  request time, only when a scope applies, so expiry is exact and needs no write. A
  **sweeper** then clears expired rows with a normal write (audit
  `maintenance.expired`, actor `system`) so the database, UI and other instances
  agree. Every instance runs it; clearing a cleared row is a no-op.
- **Response**: 503, `Retry-After`, `Cache-Control: no-store`, `X-Request-Id`, a
  body chosen by `Accept` (JSON `{"error":{"code":"maintenance",...}}`, HTML or
  text). The built-in page HTML-escapes the message.
- **Bypass**: up to 32 source CIDRs and/or one secret header. The source is the
  effective client address (the peer, or `X-Forwarded-For` only from a trusted
  proxy). The secret is stored as SHA-256, compared in constant time, shown once
  (server-generated if not given). A bypassed request is served and gets
  `X-Maintenance-Bypass: <scope>`; use is counted, not logged per line.
- **Placement**: after host and subsystem match and the 421 and redirect checks,
  before routing for global and subsystem; the route scope after the route matched;
  always before an endpoint is selected, so maintenance touches no endpoint, retry
  budget or health state. Upgrade requests get the 503; already-upgraded
  connections stay open. Health checks keep running; the admin plane is never in
  maintenance. The decision is one pure function in `routing`
  (`check(scope, now, client_ip, header) -> Allow | Bypassed | Respond`), shared by
  the data plane and the admin resolver, and exempt-able for ACME HTTP-01 (M4).
- **Metrics**, labels from configuration names only:
  `fastproxy_maintenance_active{scope,subsystem,route}`,
  `fastproxy_maintenance_responses_total{scope,subsystem,route}`,
  `fastproxy_maintenance_bypass_total{scope,subsystem}`. Maintenance 503s are
  flagged `maintenance=<scope>` in the access log and subtracted from the
  dashboard's error rate.

## Defaults

| Setting | Default |
|---|---|
| TTL | none in the API; the UI and CLI offer one by default |
| `retry_after_secs` | derived from the TTL, else 120 |
| Built-in page | message escaped, no custom body |

## As built

- The compiled form (`CompiledMaintenance`: parsed bypass rules, the prepared
  HTML, text and JSON message, the settings) lives in the routing snapshot beside
  the configuration, from `compile_all`, which takes the maintenance rows as an
  extra input because maintenance is not part of `Config`. A scope already over
  at compile time is left out; one that ends later is still judged by the clock on
  each request. A scope naming a subsystem or route that does not exist is a
  compile error.
- The decision is `CompiledMaintenance::check(now, client, header_value)`; the
  data plane and `POST /api/resolve` both call it.
- The effective client is `TrustedProxies::effective_client`: the peer, or, when
  the peer is trusted, the nearest `X-Forwarded-For` entry from the right that is
  not itself trusted.
- The traffic sampler subtracts the maintenance answers of a subsystem from its
  5xx count, so the dashboard error rate stays at zero during planned maintenance.
- The sweeper runs every second; each sweep is one read of the maintenance table.

## Consequences

- With nothing active the new step reads three `None`s: no clock read, no
  allocation, no atomic.
- Clock skew between instances moves the end by the skew; documented (NTP).
- Maintenance takes a *site* out with an explanation; endpoint drain and disable
  (M2) remain the tool for a *backend*.
