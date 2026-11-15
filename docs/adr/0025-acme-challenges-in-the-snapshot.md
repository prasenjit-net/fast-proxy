# ADR 0025: HTTP-01 challenges travel in the snapshot

Status: proposed (2026-11-15); pending the owner's review. Built in PR2. Plan reference: M4 K8,
K9.

## Context

In a cluster any instance may receive the CA's challenge request, but only the lease holder
talks to the CA. The data plane never reads the database. M5 and M6 fixed that the challenge
bypasses maintenance and the HTTPS redirect, and is never sampled.

## Decision

- **HTTP-01 only.** TLS-ALPN-01 is a stretch task; DNS-01 (and so wildcards) is post-1.0.
- Challenges (token, key authorization, the hosts it answers for, an expiry of 1 hour) are rows
  in `acme_challenge`, written through `ConfigService::apply` as a **summary-only revision** with
  an audit row (actor `system`). They are not `Config` and a rollback never touches them. They are
  compiled into the snapshot, so the writing instance answers at once and the others after their
  next sync poll.
- **Propagation wait, then a self-check.** Before the CA is told to validate, the runner waits
  `propagation_wait` (default twice `config_poll_interval` plus 2 s; 0 with the poll off), then
  fetches each challenge URL through its public name a few times. A failed self-check fails the
  order locally and spends no CA attempt.
- **Two revisions per order**: publish, then finish (the certificate write clears the order's
  challenges in the same apply; a failure clears them in its own).
- **Answering:** first in `resolve`, before the redirect, the 421 rule and maintenance. Only
  `GET`/`HEAD` of `/.well-known/acme-challenge/<token>` with a known token **for that host** is
  answered (`200 text/plain`, `Cache-Control: no-store`); anything else falls through to normal
  routing, so a backend that runs its own ACME keeps working. Answered on plain HTTP and HTTPS,
  never forwarded, sampled, retried or counted as a route; it has its own metric.
- The key authorization is public by design (the CA fetches it over plain HTTP), so it is not a
  secret and may sit in the snapshot and the revision summary.

## Consequences

- No new cross-instance mechanism: the M3 sync poll carries challenges like everything else.
- A normal request pays one prefix comparison.
- Renewals add revisions; a burst of them can push configuration revisions out of the
  100-revision rollback window. Jitter and one order at a time spread them; if it proves a
  problem, summary-only revisions stop counting toward the 100 (a store change).
