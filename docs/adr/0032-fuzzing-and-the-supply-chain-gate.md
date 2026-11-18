# ADR 0032: fuzzing and the supply-chain gate

Status: the supply-chain gate (L14) is **built** in M7 PR1 (`m7/pr1-decisions`); fuzzing (L13) is accepted
(2026-11-16, the owner). Plan reference: M7 L13, L14, tasks F1 to F5.

## Decision: the supply-chain gate (built)

- **`cargo deny check`** runs as a CI job on every pull request, configured in `deny.toml`:
  - **advisories:** RustSec; vulnerabilities and unsound crates fail; unmaintained crates fail for
    direct dependencies;
  - **licenses:** an allow-list of permissive licenses; the workspace's own unpublished crates
    are exempt;
  - **bans:** `openssl`, `openssl-sys` and `native-tls` are banned (TLS is rustls only, ADR 0009);
    no wildcard versions; duplicate versions are reported as warnings;
  - **sources:** crates.io only.

  The action is pinned by commit, at the newest release at least two weeks old.
- **No `unsafe` code.** `[workspace.lints.rust] unsafe_code = "forbid"`, and every crate opts in
  with `[lints] workspace = true`. A lint rather than a `#![forbid]` in each crate root, so tests,
  examples and build scripts are covered too. `tests/workspace_lints.rs` fails when a crate does
  not opt in. No crate used `unsafe` before.
- **What the first run found and how it was fixed:**
  - RUSTSEC-2026-0459, the SQLite bundled with `libsqlite3-sys` 0.35. Fixed by moving to
    `rusqlite` 0.43 (`libsqlite3-sys` 0.40), now one workspace dependency because SQLite can be
    linked only once. No code changes were needed.
  - RUSTSEC-2026-0386, unsound `AsyncFd` constructors in `tokio` 1.53.1. Fixed by updating to
    1.54.1.
  - The root package was publishable, so its path dependencies counted as wildcards. It is now
    `publish = false` (released as binaries), with `license = "MIT"`.

## Decision: fuzzing (proposed)

- `cargo-fuzz` (libFuzzer, nightly) in a `fuzz/` crate **outside** the workspace, so stable
  builds, `cargo test` and Windows are untouched. Targets call pure functions only (M7-PLAN
  section 5.4, fourteen targets from host patterns to the redactor and the limiter). Functions
  that are private today become reachable behind a `fuzzing` feature, never as public API.
- Each target runs 20 s on every pull request (Linux), and 20 minutes nightly; a nightly crash
  opens an issue with its input. (60 s at first; cut to 20 s because fourteen targets run one after
  another and made this the slowest CI job. Shallow bugs show in well under a second, the gcra one
  did, and the nightly run is for deep ones.) A crash input is committed as a unit test in the crate that owns
  the function. Before 1.0, every target runs one CPU hour without a crash or timeout.

## Consequences

A new dependency with a non-permissive license, a crate from outside crates.io, or a known
vulnerability fails CI. Duplicates stay visible as warnings without blocking.
