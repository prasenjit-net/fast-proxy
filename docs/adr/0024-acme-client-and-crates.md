# ADR 0024: the ACME client and the crates that hold ACME

Status: proposed (2026-11-15); PR1 builds the pure half on it, pending the owner's review. The
client library is confirmed or replaced by spike G2 at the start of PR3. Plan reference: M4 K1,
K2, K16.

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
