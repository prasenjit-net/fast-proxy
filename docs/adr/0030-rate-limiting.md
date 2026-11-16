# ADR 0030: rate limiting

Status: proposed (2026-11-16); pending the owner's review. Plan reference: M7 L2 to L6. Spike G3
measures the key table before PR2.

## Decision

- **Algorithm: GCRA** (the generic cell rate algorithm, equivalent to a token bucket). Each key
  holds one `AtomicU64`, the theoretical arrival time, updated by compare-and-swap. It needs no
  refill timer and no lock, and gives an exact `Retry-After`. A key whose time has passed answers
  exactly like a new key; the key table relies on that. The sampler's token bucket
  (`fast-proxy-sampling/src/bucket.rs`) moves to the new crate `fast-proxy-limits`, keeping its
  rule for clock readings that arrive out of order, so there is one implementation.
- **The key table** is bounded and sharded:
  - 64 shards, each a small map behind a mutex held for one lookup;
  - total capacity `[limits] rate_limit_keys`, default 100,000;
  - keys stored as 64-bit hashes with a per-process random seed, never the address itself.

  A full shard first evicts entries whose time has passed, which changes no answer. If none has
  passed, the request is **admitted** and counted as `rate_limit_table_full`. Failing open stops
  an attacker who fills the table from turning it into a denial of service for everyone. A table
  that merged colliding keys was rejected, because it would limit innocent clients.
- **Limits are per instance.** With N instances the effective limit is up to N times the policy.
  The UI, API and docs say "per instance" next to every rate. A shared budget would put the
  database on the request path.
- **Keys:**
  - `client`: the effective client address. `X-Forwarded-For` counts only from `trusted_proxies`,
    as for the maintenance bypass. IPv6 clients are keyed by their /64 by default
    (`ipv6_prefix`, 48 to 128).
  - `header:<name>`: the value of one request header, hashed, never stored, logged or sampled
    raw. It falls back to the client address when the header is missing. The header is added to
    the sampler's redaction list automatically.
- **Order:** after the host and route matched, and after the ACME challenge, the redirect, the 421
  rule and maintenance; before endpoint selection. So the challenge is never limited, and an
  answer that costs nothing upstream is not counted. A route policy **replaces** the subsystem's
  policy for that route; one request means one lookup.
- **Answer:** a fixed `429` with `Retry-After` in whole seconds, rounded up. The draft
  `RateLimit` and `RateLimit-Policy` headers are not sent in 1.0, because the draft still changes.
- **Policy** (validated in `model`):
  - `rate` at least 1, per `per_secs` of 1 to 3600;
  - `burst` from 1 to 10,000;
  - the key, where a header name must be a token, and not a hop-by-hop or pseudo header.

  Stored in a new `rate_limit_policy` table. That is schema 7, additive, with its migration
  (ADR 0028).
- **State across a configuration swap:** tables are keyed by policy identity (subsystem name,
  plus the route prefix) and carried across snapshot swaps like endpoint runtime state (ADR
  0005). A reload does not reset a client's budget. A removed policy's table is dropped.

## Consequences

Metrics `fastproxy_rate_limited_total{subsystem,route}` and
`fastproxy_rate_limit_table_full_total`, with labels from configuration names only. Admin writes
go through `ConfigService::apply` and are audited (`rate_limit.set`, `rate_limit.clear`). Exit
criterion: admitted traffic within 5% of the policy over 10 s, and the table never past its
capacity with a million distinct clients.
