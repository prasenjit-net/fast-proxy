# ADR 0011: certificates in the snapshot, the cache, the write path and cluster readiness

Status: accepted. Plan reference: M3 F7, F8, F9, F10, F21.

## Context

A handshake must never see a new HTTPS setting with an old certificate, a
renewal must cost one parse rather than N, and several instances sharing a
database (later PostgreSQL) must converge on the same certificates.

## Decision

- **One atomic swap (F7).** The compile step takes certificate handles and the
  process TLS state as inputs; each `CompiledSubsystem` carries its `https`
  settings and certificate handle. The SNI resolver (in the data plane) loads the
  snapshot, finds the subsystem and returns its handle. The compile veto also
  rejects HTTPS enabled with no usable certificate while TLS is on.
- **`CertCache` (F8)** holds parsed, decrypted rustls keys keyed by id and
  version (`updated_at` plus fingerprint). A rebuild reuses unchanged entries,
  parses new or changed rows and drops removed ones. Replaced handles live on
  through `Arc` until their handshakes finish; established connections are
  untouched. Decrypted keys exist only in the cache. An unreadable row is marked
  and reported; its subsystems get the ephemeral certificate and 421.
- **One write path (F10).** Certificate and HTTPS writes go through
  `ConfigService::apply`: lock, transaction (seal keys, write rows), reconcile
  dev certificates, veto by compiling the would-be snapshot with the would-be
  certificates, commit with a revision, swap, broadcast. No file watcher and no
  reload endpoint.
- **Cluster readiness (F9), built for one instance.** The revision version is
  the cluster generation. A sync poll (`config_poll_interval`, default 5 s, 0
  disables) compares the database version with the live snapshot's; when it is
  ahead it reloads configuration and certificates and swaps, reusing runtime
  state and the cache. Instances are eventually consistent within the interval.
  Certificate reads sit behind a narrow `CertificateSource` trait with portable
  SQL so PostgreSQL can implement it later. Instance-local by design: endpoint
  health and ejection, in-flight counts, session-ticket keys. The master key must
  be identical everywhere.
- **Crates (F21).** New `fast-proxy-tls` (PEM, vault, generator, `CertHandle`,
  `CertCache`, rustls configs) depends on `model` and `telemetry`. `routing`
  depends on `tls` for the handle type (data only, no I/O). `store` has no crypto
  dependency; `config-service` depends on `tls`. Dependencies point one way.

## Consequences

- Handshakes and the 421 rule (ADR 0012) compare ids from one snapshot.
- `routing` is no longer free of the `tls` crate, but still has no I/O.

## Alternatives considered

- A separate certificate swap beside the snapshot: a handshake could see a new
  setting with an old certificate.
- A file watcher or reload endpoint: a second write path to keep consistent.
