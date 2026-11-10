# ADR 0005: runtime state and stable endpoint identity

Status: accepted. Plan reference: M2 E1, E2, E3.

## Context

M2 adds per-endpoint runtime state (in-flight count, health, ejection, EWMA
latency). Configuration is an immutable `Snapshot` that is swapped on every
change, while runtime state must survive those swaps: an ejected endpoint must
stay ejected when an unrelated route is edited. M1 re-inserts endpoints without
their ids on rollback, so ids are not stable today.

## Decision

- Endpoints carry a stable `EndpointId` in `Config` and in revision JSON. Rollback
  restores ids exactly.
- Runtime state lives in an `Arc<EndpointRuntime>` (atomics only) held by each
  compiled endpoint. `compile` takes a `RuntimeRegistry` and reuses a runtime
  when the endpoint id **and** address are unchanged; a changed address or a
  removed endpoint starts fresh. There is no global map on the request path.
- New pure crate `fast-proxy-balancing` (depends on `model` only) owns runtime
  state, selection and the passive-health state machine. New crate
  `fast-proxy-health` owns active probes (I/O). The data plane still never
  touches the store.

## Consequences

- The hot path reads atomics through a pointer it already holds; no lock, no
  lookup.
- `compile` stays deterministic for config and gains one explicit input.
- Rollback and runtime carry-over are tested together (stale-state risk).

## Alternatives considered

- A global `DashMap` keyed by id: a lookup and shared contention per request.
- State inside the immutable config: lost on every swap.
