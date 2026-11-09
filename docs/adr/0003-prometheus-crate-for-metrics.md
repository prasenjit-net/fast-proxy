# ADR 0003: the `prometheus` crate for metrics

Status: accepted (owner confirmed 2026-11-09). Plan reference: M1 D6.

## Context

M1 exposes baseline metrics at `GET /metrics` on the admin listener. Label
values must come only from config names (never raw paths or hosts) to keep
cardinality bounded.

## Decision

Use the `prometheus` crate: counters, gauges, and histograms with a built-in
text exposition encoder.

## Consequences

- Smallest path to a correct exposition format.
- Metrics live in `fast-proxy-telemetry` behind our own small types, so the
  library can be swapped (for example to the `metrics` facade) without
  touching the data plane.
- Revisit if the spike shows hot-path overhead from label lookups; the
  mitigation is caching resolved label handles in the compiled snapshot.
