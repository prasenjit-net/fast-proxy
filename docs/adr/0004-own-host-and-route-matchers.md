# ADR 0004: own host and route matchers

Status: proposed. Plan reference: M1 D7.

## Context

Routing is host to subsystem, then longest path prefix on segment boundaries
to a route. It runs on every request against an immutable snapshot.

## Decision

Implement the matchers ourselves in `fast-proxy-routing`: a `HashMap` for exact
hosts, a suffix `HashMap` for wildcard hosts (longest suffix wins), and a
segment-aware longest-prefix lookup for routes.

## Consequences

- Semantics (segment boundaries, exact beats wildcard) are exactly ours and
  covered by property tests against a brute-force reference.
- No dependency to audit for a few hundred lines of code.
- If the benchmark in Q3 shows routing is a bottleneck with large route
  tables, evaluate a radix-tree crate then, not before.

## Alternatives considered

- `matchit` or `route-recognizer`: designed for path templates with
  parameters, not host plus segment-boundary prefix semantics.
