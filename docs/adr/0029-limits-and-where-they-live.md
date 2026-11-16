# ADR 0029: request and connection limits, and where each one lives

Status: proposed (2026-11-16); pending the owner's review. Plan reference: M7 L1, L7, L9, L10,
L11. Nothing is built yet; spike G2 settles the slow-client details before PR3.

## Context

Until M7 the data plane bounds only the request head: a 10 s header-read timeout and a 64 KiB
head buffer, both constants in `server.rs`. On TLS there are also the handshake timeout and
concurrency cap (M3), and on HTTP/2 the stream and header-list limits with the rapid-reset caps.
Nothing bounds how many connections one client holds, how slowly it sends a body or reads a
response, or how large a body is.

## Decision

- **Two homes.**
  - **Process limits** go in a new `[limits]` section of `config.toml` (restart to change):
    connection caps, connection-level timeouts, header and URI limits, the rate-limit table's
    capacity, and the default body cap. They protect the process, so they sit with the listeners.
  - **Traffic limits** go in the database as `Config`: rate-limit policies, per-route body caps,
    per-group endpoint caps. A revision holds them and a rollback restores them, because a
    rolled-back route should come back with its limits.
- **Connection caps** are checked when a connection is accepted, before the TLS handshake:
  - `max_connections`, default 10,000, across all listeners, upgraded tunnels included;
  - `max_connections_per_client`, default 256, by peer address; `0` turns it off, for use behind
    a load balancer.
  A refused connection is closed at once and counted by reason. At the global cap the accept
  loop waits 10 ms per refusal so it does not spin.
- **Timeouts measure lack of progress, not total time:**

  | Key | Default | What it bounds |
  |---|---|---|
  | `header_read_timeout` | 10 s | the existing constant, now configurable |
  | `idle_timeout` | 60 s | keep-alive with no request in flight |
  | `body_read_timeout` | 30 s | no progress reading a request body; `408` if no response has started |
  | `send_timeout` | 60 s | no progress writing to the client |
  | `tunnel_idle_timeout` | 1 h | upgraded connections; `0s` turns it off |

  The body and send timeouts come from a `ProgressIo` wrapper that records the last read and
  write instants, checked by one timer per connection.
- **Size limits** answer with fixed responses:
  - `max_header_count` (100) and `max_header_bytes` (64 KiB): `431`;
  - `max_uri_bytes` (8 KiB): `414`;
  - `max_body_bytes`: `413`. The process default is 0, meaning unlimited, so no existing upload
    changes. A route can set its own cap. A `Content-Length` over the cap is refused before the
    upstream is contacted. A streamed body that crosses the cap is cut, the upstream request
    aborted, and the client gets `413` if no response has started. A cut body is never
    replayable (the ADR 0008 rule).

  These numbers are the same table as the M6 "unusual headers" thresholds, so a request is
  refused or sampled by one rule set.
- **Framing (request smuggling)** is refused with `400` before anything is forwarded:
  - both `Content-Length` and `Transfer-Encoding`;
  - more than one `Content-Length`;
  - a `Transfer-Encoding` other than a final `chunked`;
  - header names or values HTTP/1.1 forbids;
  - for HTTP/2 requests forwarded over HTTP/1.1, connection-specific headers and CR or LF in
    values.

  The checks are pure functions in `headers.rs`, unit-tested and fuzzed.
- The HTTP/2 limits stay in `[data_plane.tls]`, where M3 put them.

## Open for spike G2

Which of these hyper 1.11 already enforces: whether `header_read_timeout` covers the idle time
between keep-alive requests, and whether `http1().max_headers` covers the header count. And what
`ProgressIo` costs, with a budget of 1% on the M6 benchmark.

## Consequences

Every refusal is a fixed response with a reason in the access log, a counter and a sample (the
sampler sees them as 4xx). The defaults are generous, and the body cap is off, so an upgrade does
not refuse traffic it used to forward.
