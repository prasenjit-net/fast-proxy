# ADR 0028: schema changes are additive and migrated

Status: accepted (2026-11-16, the owner's rule; this ADR records how it is built). Plan
reference: M7 L18 and task C1. Replaces the "no migrations while undeployed" rule of M1 (D2) and
of the ROADMAP decision log of 2026-11-09.

## Context

Until v0.1.1 every schema change edited `schema.sql` in place and a database of another version
was refused, which was fine while nobody ran the proxy. v0.1.1 is released, and the next release
adds the ACME tables (schema 6). Without an upgrade path a v0.1.1 database would no longer open.

## Decision

- **Additive changes only.** New tables, and new columns that are nullable or have a default. A
  drop, rename or type change needs a step that carries the data over, in SQL, in the same
  transaction.
- **One step per version** in `crates/fast-proxy-store/src/migrations/` (`v<N>_<topic>.sql`),
  listed in `STEPS`. A test checks the steps lead from `OLDEST_UPGRADABLE` (5, v0.1.1) to
  `SCHEMA_VERSION` with none missing.
- **`schema.sql` is the schema of a new database** at the latest version. A column a step adds goes
  at the end of its table there too, so a new database and an upgraded one have the same shape.
  A test compares them: the columns in order with types, defaults and keys, foreign keys,
  implicit and named indexes, and `CHECK` clauses. The SQL text is not compared, because SQLite
  stores `ADD COLUMN` by splicing text into the original statement.
- **At open:** a file at an older, upgradable version is copied, then upgraded. All missing steps
  and the new `user_version` run in **one `IMMEDIATE` transaction**. The version is read again
  under the write lock, so when several instances start on one file at once, the first upgrades
  it and the others find it current. A failed step rolls everything back, and the file stays at
  its old version. Creating a new database's schema takes the same lock.
- **The copy:** `<db>.pre-v<N>-<unix ms>.bak`, written with `VACUUM INTO` (a consistent copy while
  WAL is on) into a file created with mode 0600. The name is claimed with an exclusive create.
  Older copies beyond the newest three are removed, but never one younger than an hour, so one
  instance cannot remove a copy another is still writing. If the copy cannot be written, the
  upgrade does not start. The copy holds what the database holds; the operator guide says to
  treat it like the database.
- **Never downgrade.** A file newer than the binary, or older than `OLDEST_UPGRADABLE`, is refused
  as before (`IncompatibleSchema`), and the message says which versions this build reads. A
  read-only open (`config validate`) does not upgrade either.
- **Report it.** Startup logs the upgrade once, with the copy's path, at `warn`.

## Consequences

- Every schema change from now on is a step plus a test. The fixture
  `migrations/fixtures/schema_v5.sql` is v0.1.1's `schema.sql` as released. Later releases add
  their own fixture when a step needs one.
- Instances sharing a file must be upgraded together: an old binary refuses the upgraded file,
  which is the existing `user_version` check.
- A data-plane process is not affected: the upgrade runs before the store thread starts, as the
  schema creation always did.
- The upgrade test against real released binaries (M7 PK3) builds on this.
