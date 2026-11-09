# ADR 0007: health checks and outlier ejection

Status: accepted. Plan reference: M2 E6, E7, E15.

## Context

Unhealthy endpoints must leave rotation automatically and return on recovery,
without flapping and without ejecting a whole group.

## Decision

- Passive ejection is on by default: 3 consecutive failures, base ejection
  10 s doubling to a 5 min cap, never more than 50% of a group; after expiry one
  half-open trial request decides recovery.
- A passive failure is a connect error, a response timeout, or `502/503/504`
  from the upstream. A plain `500` is not a health signal.
- Active checks (HTTP or TCP) are off by default; when enabled: interval 5 s,
  timeout 2 s, rise 2, fall 3, expected HTTP 200-399. Both mechanisms feed one
  routability decision: active healthy AND not ejected.
- New endpoints start healthy (optimistic) and are demoted after `fall` failed
  probes. A restart rebuilds runtime state from scratch.
- State machines take `now` as a parameter; no hidden clock.

## Consequences

- Tests are deterministic and need no sleeping.
- An application that returns `500` for everything stays in rotation; that is
  an application problem, not an endpoint outage.

## Alternatives considered

- Counting `500` as failure: ejects healthy hosts on application bugs.
- Starting unhealthy until probed: a window of 503s on every config change.
