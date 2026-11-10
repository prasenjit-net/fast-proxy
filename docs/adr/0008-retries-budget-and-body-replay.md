# ADR 0008: retries, retry budget and body replay

Status: accepted. Spike G2 confirmed E12 (findings below). Plan reference: M2 E10, E11, E12.

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
- Body replay (E12): `Incoming` cannot be cloned and the pooled client takes the
  body by value, so the data plane keeps the body behind a shared pointer
  (`BodyOwner`) and gives each attempt a `ReplayBody` view. A request is
  replayable while no attempt has polled the body; a request with no body is
  always replayable.
- Refinement of E10: besides connect failures and idempotent 502/503/504, an
  idempotent request is also retried when the connection broke after the
  request was handed over (`reset`). A non-idempotent request is retried only
  after a connect failure, even if its body was never polled, because hyper does
  not say whether the head reached the peer.
- The budget is kept per compiled group, so a configuration change starts it
  empty (at most ten seconds of history are lost).

## Findings (spike G2, pinned by tests in `replay_body.rs`)

a. **The hyper-util legacy client does not poll the request body before it has
   a connection.** After a connect failure (`Error::is_connect()` is true) the
   body is untouched and the next attempt sends all of it.
b. **A peer that drops the connection after the request was sent** gives an
   error that is not a connect error, and the body has been polled, so the
   request is not replayable. A reused pooled connection closed by the peer
   cannot be provoked deterministically; such failures surface as non-connect
   errors and are handled by the conservative rule above.
c. **The shared-pointer wrapper works**: reclaim after a connect failure, no
   replay after the first poll, an empty body is always replayable.

## Consequences

- A failing group cannot be hammered by its own retries.
- Retried requests with bodies are safe: they are sent in full once, or not at
  all, to an endpoint that never saw them.
- Upgrades get exactly one attempt.
