# ADR 0021: redaction and body capture in samples

Status: accepted by the owner (2026-11-11); J15 confirmed by spike G2 (see the result below). Plan
reference: M6 J8, J9, J15.

## Context

A sample holds request data, which can hold secrets. Redaction must be on by default, bodies
must be opt-in, and neither may cost the request path.

## Decision

- **Redaction happens once, in the sampler task, before a sample is stored anywhere**, so
  nothing raw is ever at rest: not in the database, a copy of it, an export or a log.
- **Headers.** Always redacted: `authorization`, `proxy-authorization`, `cookie`,
  `set-cookie`, `x-api-key`, `x-auth-token`, `x-csrf-token`, `x-amz-security-token` and the
  bypass header name of every maintenance scope, plus the operator's additions. The value
  becomes `[redacted]`; the name stays.
- **Query string.** Parameters named `token`, `access_token`, `refresh_token`, `id_token`,
  `key`, `api_key`, `apikey`, `secret`, `password`, `passwd`, `signature`, `sig`, `code`,
  `session`, `ticket`, or an addition, become `[redacted]`.
- **Bodies.** A **lexical** redactor (it must work on a truncated prefix, so parse and
  serialize is wrong) blanks the values of JSON members and form fields named `password`,
  `passwd`, `secret`, `token`, `access_token`, `refresh_token`, `client_secret`, `api_key`,
  `authorization`, `card`, `cvv`, `ssn`, or an addition. If a body cannot be redacted with
  confidence (an unterminated string at the cut, an unknown encoding) the unsafe tail is
  dropped, not kept.
- **Additions only.** Settings, subsystem overrides and route overrides can add names; none
  can remove a built-in one. Redaction lists live only in the global settings row.
- **Bodies are off by default and opt in per route** (a subsystem override turns them on for
  its routes). Only text content types (`text/*`, `application/json`, `application/xml`,
  `application/*+json`, `application/*+xml`, `application/x-www-form-urlencoded`) with no
  `Content-Encoding` or `identity`; never for upgrades. The request body is teed on the body
  owner (so a replayed attempt is never captured twice: a body is replayable only if it was
  never polled, ADR 0008) and the response body on the tracked body, each up to a cap.
- **Memory.** A teed body is held until the decision. Every reserved byte counts against a
  process-wide budget; when it is spent the request is not captured and the sample says
  `bodyOmitted: budget`. Bodies of samples that did not trigger are dropped.
- **Selected headers.** The request side keeps a small allow-list (`host`, `user-agent`,
  `content-type`, `content-length`, `accept`, `accept-encoding`, `referer`,
  `x-forwarded-for`, `x-request-id`, `origin`) plus additions, not the whole header map.
  Spike G2 measures this against a full clone before the choice is final; the fallback keeps
  the head only when a request-time flag or the up-front baseline draw says so.

## Defaults

| Setting | Default |
|---|---|
| Body capture | off everywhere |
| Request and response body caps | 4 KiB each, at most 64 KiB |
| Body memory budget | 16 MiB per process |
| Headers kept per sample | at most 64 of 1 KiB |
| Path and query | at most 8 KiB |

## Consequences

- Residual risk, stated in the docs and the UI: a secret in a field nobody listed, in a path
  segment, or in free text is not found.
- The redactor is pure, so it is covered by property tests and is a fuzz target in M7.

## Spike G2 result (2026-11-12): the cost of the request head and of a body tee

A throwaway release-mode benchmark (a standalone crate, not kept) on one laptop (macOS, APFS, a
shared machine, so small differences are noise). Request headers were 14 typical headers whose values
share one buffer, as hyper's do. For scale, 97k requests per second over 64 connections is roughly
10,000 ns of CPU per request.

| Per request | Cost |
|---|---|
| Nothing retained | under 1 ns |
| Full `HeaderMap` clone | about 100 ns |
| The 10 allow-listed headers into a `Vec` of pairs | about 100 ns |
| The 10 allow-listed headers into a fixed stack array | about 77 ns |
| Capped body tee (reserve budget, allocate 4 KiB, copy up to the cap over 16 frames, release) | about 60 ns, only on routes that opted in |

- **J15 confirmed, with one refinement:** keep the allow-list, held in a **fixed stack array of
  header values** (no `Vec`), and take it **only for routes whose sampling is on**. That is about
  0.8% of the per-request CPU at the benchmark's rate, under the 1% fallback threshold. A full
  clone costs about the same, so the allow-list is chosen for what it keeps (less retained data,
  no secrets held that nobody needs), not for speed. The fallback (keep the head only when a flag
  or the baseline draw says so) is not needed. The end-to-end figure is still measured by Q6.
- **The tee is cheap enough** to be a documented cost of opting in. That a retried request is
  never captured twice follows from the body owner's `polled` flag (a replayable body was never
  read) and is covered by task D2's tests; it was not exercised in the spike.

## Found while building (M6 PR8)

`Referer` and `Location` are in the fixed set of kept headers and hold URLs, so the query inside
them is redacted with the same parameter rules as the request's own. The known limits (a secret
in an unlisted field, in the path, or in free text; the access log) are listed in
[../security-review-m6.md](../security-review-m6.md).
