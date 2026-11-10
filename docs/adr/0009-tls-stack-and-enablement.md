# ADR 0009: TLS stack and the enablement model

Status: accepted. Plan reference: M3 F1, F3, F4, F12, F19, F20.

## Context

M3 makes the proxy terminate TLS. We need a crypto stack that builds on Linux,
macOS and Windows CI, a clear on/off model, and bounded behaviour under hostile
handshakes and HTTP/2 traffic.

## Decision

- **Stack:** `rustls` with the `ring` provider (`tokio-rustls` for the server,
  `hyper-rustls` for upstreams). `ring` needs no C toolchain beyond what the
  workspace already builds; `aws-lc-rs` needs cmake (and NASM on Windows). The
  provider is one line to swap.
- **Two-level switch.** Process level: `[data_plane.tls] enabled` (default
  `false`), overridden by `--tls` / `--no-tls`. When on, exactly one HTTPS
  listener starts (`--https-listen`, default `0.0.0.0:8443`) next to the plain
  listeners, which keep serving. When off there is no TLS listener and no TLS
  cost; subsystem HTTPS settings stay stored but inert (`effective: false`).
  Subsystem level: `https.enabled` (default `false`).
- **Policy:** minimum version `1.2` (default) or `1.3`; rustls default cipher
  suites; ALPN `h2` and `http/1.1` (`http2 = false` removes `h2`); session
  tickets on; early data (0-RTT) never.
- **HTTP/2 to clients only.** Upstream connections stay HTTP/1.1 (over TLS when
  the group says so): the pool, retry and body-replay code assumes HTTP/1.1, and
  multiplexing upstream would change in-flight accounting. Revisit with
  benchmark data (task C4).
- **Handshake safety:** 10 s handshake timeout and at most 256 concurrent
  handshakes; failures are counted by reason, never one info-level line each.
- **HTTP/2 hardening:** explicit limits (128 concurrent streams, header list
  size, windows, keep-alive ping with timeout), rapid-reset mitigations on,
  GOAWAY on graceful shutdown. All are config-file knobs with safe defaults.

## Spike G2 findings

Recorded here when the spike (rustls + ring + hyper h2 + AES-GCM + snapshot
carried certificates, on all three CI platforms) is done. Until then F1 stands
as a recommendation backed by the toolchain argument above.

## Consequences

- TLS off is the default and costs one branch in the accept path.
- Plain and TLS listeners coexist, so redirects (ADR 0012) are possible.

## Alternatives considered

- `aws-lc-rs`: faster in places, heavier to build on Windows.
- Per-listener `tls = true`: dropped; a single process switch is simpler and the
  subsystem decides what is served over HTTPS.
- HTTP/2 upstream now: deferred, no data yet.
