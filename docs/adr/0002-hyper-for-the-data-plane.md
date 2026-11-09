# ADR 0002: hyper and hyper-util for the data plane

Status: accepted (spike confirmed, 2026-11-09). Plan reference: M1 D5.

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
- The pooled client from `hyper-util` is sufficient; the planned fallback (a thin
  pool over `hyper::client::conn`) is not needed.

## Spike results (P1)

Verified by the forwarding tests in `fast-proxy-dataplane`:

- **Pool reuse:** five sequential requests used one upstream connection.
- **Timeouts:** the connect timeout is a property of the connector, not of a
  request, so the client keeps one pool per distinct connect timeout. The
  response timeout is applied per request with `tokio::time::timeout` (504).
- **No buffering:** a 64 MiB request body streams through; the first half of a
  request body reaches the upstream while the client is still sending the
  second half; the first chunk of a chunked response reaches the client while
  the upstream body is still open.
- **Cancellation:** when the client disconnects, the upstream request is
  dropped within the test's 3 second window.
- **Framing:** chunked request bodies and `Expect: 100-continue` work.

Not covered: a connect timeout against an unroutable address (only the error
classification is unit tested), and a memory ceiling check with a body of
hundreds of MB.
