# ADR 0031: per-endpoint request caps

Status: proposed (2026-11-16); pending the owner's review. Plan reference: M7 L8. Deferred from M2
(per-endpoint connection limits).

## Decision

- An upstream group may set **`max_requests`**, the most requests one endpoint has in flight. It
  is unset by default, meaning unlimited. It is stored as a nullable column on `upstream_group`
  (schema 7, with its migration) and is part of `Config`.
- It is enforced with the in-flight counter `EndpointRuntime` already keeps (ADR 0005). An
  endpoint at its cap is **ineligible** for selection, like a draining or disabled one, and the
  balancer moves to the next endpoint the algorithm would pick.
- When every eligible endpoint is at its cap, the request gets a fixed `503`, reason
  `endpoint_capacity`. There is never a queue.
- Retries respect caps, and panic mode (M2) does not override one: a cap protects the upstream
  even when health checks say everything is down.

## Consequences

The rule "draining and disabled endpoints are never selected" gains a third reason. The selection
tests of ADR 0006 run with caps too. The cap counts requests rather than pooled connections,
because requests are what the proxy controls; the connection pool still reuses idle connections
freely.
