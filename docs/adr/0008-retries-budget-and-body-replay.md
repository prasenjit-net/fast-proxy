# ADR 0008: retries, retry budget and body replay

Status: accepted for E10 and E11; **E12 (body replay) is provisional** until
spike G2 reports. Plan reference: M2 E10, E11, E12.

## Context

Killing one backend under load must not produce client errors, yet retries must
never duplicate side effects or amplify an outage.

## Decision

- Default max 2 attempts (one retry). Connect-phase failures are retried for any
  method (the request was never sent). Idempotent methods (`GET`, `HEAD`,
  `OPTIONS`, `PUT`, `DELETE`, `TRACE`) are also retried on `502/503/504`.
  Timeouts are opt-in. Upgrades are never retried, nor is any request after a
  body byte was sent. A retry excludes the endpoint that just failed.
- Per-group retry budget: retries may not exceed 20% of recent requests, with a
  floor of 10 retries per second.
- Body replay (E12): `Incoming` cannot be cloned. A small lendable body wrapper
  would let a connect failure reclaim a never-polled body. This depends on the
  hyper-util client not polling the body before connecting; **spike G2 verifies
  this**. If it fails, only bodyless requests are retried.

## Findings (to be filled in by spike G2)

Pending: (a) body not polled before connect, (b) error shape for a reused pooled
connection closed by the peer, (c) lendable wrapper reclaim behaviour.

## Consequences

- A failing group cannot be hammered by its own retries.
- Worst case for E12: requests with bodies are not retried; connect failures on
  bodyless requests still are.
