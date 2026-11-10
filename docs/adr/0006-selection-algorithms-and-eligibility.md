# ADR 0006: selection algorithms and eligibility

Status: accepted. Plan reference: M2 E4, E5, E8, E9.

## Context

A group has several endpoints; each request must pick one cheaply, avoid
unusable ones, and have a defined behaviour when none is usable.

## Decision

- Algorithms per group: `round_robin` (default), `weighted_round_robin`,
  `least_connections`, `p2c_ewma`, `consistent_hash` (key: `client_ip` default,
  `header:<name>` or `cookie:<name>`; no key falls back to round robin).
- Weights are integers `1..=1000`; zero is not allowed (use `disabled`). At most
  64 endpoints per group.
- Eligible = admin state `enabled` AND healthy AND not ejected.
- If none is eligible the group's failure policy applies: `fail_open` (default,
  "panic mode") picks among admin-`enabled` endpoints ignoring health;
  `fail_closed` answers `503 no healthy upstream`. `draining` and `disabled`
  endpoints are never used, even in panic mode.
- `draining` and `disabled` both stop new requests and let in-flight requests
  and open tunnels finish; they differ in meaning and display only (the UI shows
  when a draining endpoint reaches in-flight 0).

## Consequences

- A false-positive health failure cannot become a total outage by default.
- Selection is atomics-only; rings and weight tables are built at compile time.
- Budget: under 200 ns at three endpoints, a few microseconds at 64, no
  allocation.

## Alternatives considered

- Fail closed by default: safer for correctness, riskier for availability.
- One merged drain/disable state: simpler, loses the "leaving soon" signal.
