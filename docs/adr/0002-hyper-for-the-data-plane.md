# ADR 0002: hyper and hyper-util for the data plane

Status: proposed. Plan reference: M1 D5.

## Context

The data plane must stream request and response bodies without buffering,
control connect and response timeouts per route, pass upgrades through, and
later add TLS and HTTP/2. Axum is a router built for application handlers, not
a forwarding engine.

## Decision

Build the data plane on `hyper` 1.x with `hyper-util` (the auto server builder
and the legacy pooled client). Axum stays admin-only, in `fast-proxy-admin`.

## Consequences

- Full control over connections, bodies, and upgrades; HTTP/2 and TLS (M3)
  slot into the same builders.
- More code than a higher-level proxy library, but each piece is small and
  testable.
- Risk: the pooled client may lack a control we need (per-request timeouts,
  cancel on client drop). If so, fall back to a thin pool over
  `hyper::client::conn`. This is checked by the spike in the M1 plan (P1),
  which has not been run yet.
