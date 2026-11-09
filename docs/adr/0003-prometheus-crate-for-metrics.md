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

## Outcome (T1)

Implemented as planned: `Metrics` in `fast-proxy-telemetry` owns the registry
and exposes typed methods, and the same registry feeds the live traffic
sampler. Counters are labelled with config names only. Label-handle lookups on
the request path are not yet measured; the baseline benchmark (Q3) will show
whether caching resolved handles in the snapshot is worth doing.
