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

Verified so far (PR3, macOS; Linux and Windows are confirmed by CI on the PR):

- `rustls` 0.23 with `ring` (no `aws-lc-rs`, no C toolchain needed) builds and
  links together with `tokio-rustls`, `rcgen` (ring backend, PEM) and
  `x509-parser`; the workspace tests and clippy pass.
- (d) The client side works against a real TLS server: trusted, untrusted, wrong
  name, IP endpoints with and without a name override, and verification off.
- (e) `rcgen` generates certificates with exact and wildcard SANs and a chosen
  validity; `x509-parser` reads names, subject, issuer, validity back, and the
  SHA-256 fingerprint is computed from the DER.
- (f) `ring`'s AES-256-GCM seals and opens with associated data; a moved or
  tampered ciphertext is rejected.
- ALPN `h2` / `http/1.1` and TLS 1.2 versus 1.3 negotiation work through
  `tokio-rustls` with a test client.
- Keys parse as PKCS#8, PKCS#1 (RSA) and SEC1 (EC) through rustls's PEM
  support; encrypted keys are detected from their headers and rejected.

Still to verify, in the PRs that need them: (b) hyper's auto builder serving h2
and HTTP/1.1 behind the acceptor and (c) a resolver reading an `arc-swap`
snapshot while it is swapped, and (g) two SQLite connections seeing each
other's committed revision version.

Verified later: (c) the resolver reads the live snapshot per handshake and the
next handshake sees a swapped certificate (PR4b); (g) two config services, and
two real processes, on one SQLite file see each other's commits and converge
within the poll interval (PR6). Still open: (b) hyper's auto builder serving h2
behind the acceptor (D2). F1 stands.

## Consequences

- TLS off is the default and costs one branch in the accept path.
- Plain and TLS listeners coexist, so redirects (ADR 0012) are possible.

## Alternatives considered

- `aws-lc-rs`: faster in places, heavier to build on Windows.
- Per-listener `tls = true`: dropped; a single process switch is simpler and the
  subsystem decides what is served over HTTPS.
- HTTP/2 upstream now: deferred, no data yet.
