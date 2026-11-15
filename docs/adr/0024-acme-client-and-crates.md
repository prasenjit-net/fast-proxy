# ADR 0024: the ACME client and the crates that hold ACME

Status: proposed (2026-11-15), pending the owner's review; the library choice confirmed by spike
G2 (see the result below). Plan reference: M4 K1, K2, K16.

## Context

M4 adds automatic certificates through ACME (RFC 8555). The protocol needs JWS signing, nonce
handling, order polling and problem documents; the rest (which names, when to renew, how to back
off, how a cluster shares the work) is our own policy. The data plane must stay free of the
store and of any network client other than its upstream pool.

## Decision

- **Protocol client: `instant-acme`** with its `ring` feature (one crypto stack, as everywhere
  else), behind a narrow `AcmeClient` trait so the runner and the tests never see the library.
  Spike G2 checks the dependency tree (no `aws-lc-rs`, one `hyper` and `rustls`), a Windows
  build, serialisable account credentials and a run against Pebble. The fallback is our own
  minimal client on `ring` and `hyper-rustls` behind the same trait.
- **`fast-proxy-acme`** (depends on `model` only until the client arrives): the eligibility rule
  (`eligible_names`), the renewal and backoff policy (`RenewalPolicy`), the problem classifier
  and the `OrderStep` state machine. Pure: time is a parameter.
- **`fast-proxy-acme-service`** (PR3; depends on `acme`, `config-service`, `tls`, `model`): the
  runner that takes the lease, schedules, drives orders and writes every result through
  `ConfigService`.
- **`fast-proxy-tls`** gains `generate_order_key` (a fresh ECDSA key and a CSR per order),
  `validate_issued` (the upload checks plus "covers every ordered name") and
  `Vault::seal_secret` / `open_secret` for the account credentials.
- The data plane depends on neither ACME crate; it answers challenges from the snapshot
  (ADR 0025). The store has no cryptography dependency and sees the account only as ciphertext.
- **Tests** use an in-process ACME test CA (a dev-only crate) that really fetches the challenge
  from the proxy and signs the CSR with the test PKI (`TestCa::sign_csr`), on all three
  platforms; an opt-in Linux CI job runs the same test against Pebble.

## Consequences

- Swapping the client touches one adapter file.
- The policy is unit- and property-tested without a CA or a network.
- One more dependency tree (`instant-acme`) to keep current; the Pebble job catches drift.

## Result of spike G2 (2026-11-15)

- `instant-acme` 0.8.5 with `default-features = false, features = ["ring", "hyper-rustls"]`
  builds without `aws-lc-rs` and adds no duplicate `hyper`, `rustls`, `ring` or `http`.
- Account credentials serialise with serde; they are sealed whole.
- Its HTTP client is HTTPS-only, so `[data_plane.acme] directory` must be `https` everywhere (a
  private or test CA is trusted through `directory_ca`, which maps to `builder_with_root`).
- A problem document does not carry the response's `Retry-After`, so a `rateLimited` order waits
  the 1-hour default of ADR 0026.
- The adapter is a concrete type (`AcmeClient`, `AcmeSession`, `PendingOrder`) rather than a
  trait: the in-process test CA makes the real adapter cheap to test, and a library swap still
  touches one file.
- Run end to end against the in-process test CA on every platform. The Pebble run is written
  (`crates/fast-proxy-acme/tests/pebble.rs`, ignored by default) and becomes the CI job of PR4.
