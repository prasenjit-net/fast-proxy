# ADR 0023: sampling settings outside `Config`, and who can read samples

Status: accepted by the owner (2026-11-11). Plan reference: M6 J3, J12, J16.

## Context

Sampling behaviour must change live, be audited and be shared by instances, like other
settings. But some of it is security-relevant (body capture, redaction additions), and
samples hold more than the `viewer` role was promised.

## Decision

- **Process settings** in `config.toml` (restart): `[sampling] enabled`, `queue_depth`,
  `batch_size`, `batch_interval`, `body_budget_bytes`. Flags `--sampling` and `--no-sampling`.
- **Behaviour in the database, live**: one global settings row (triggers, thresholds, the
  baseline rate, the rate limit, the storm guard, body caps, redaction additions and the
  retention of the shared table) and optional **overrides per subsystem and per route**
  (enabled, body capture, baseline rate, slow thresholds). Resolution: route over subsystem
  over global; an empty field inherits.
- **Not `Config`.** Like tokens, audit and maintenance (ADR 0017, H8) the settings never
  enter a revision's configuration copy and **a rollback never changes them**: a rollback
  must not silently turn body capture on or remove a redaction addition. A change is a
  summary-only revision (so the sync poll carries it) with an audit entry (`sampling.update`,
  `sampling.override.set`, `sampling.override.clear`) written in the same transaction.
- **Access.** A new `Action::Samples` for list, detail, export and clear: **`admin` only**.
  A `viewer` can read the sampling status and settings (through `Read`; the redaction
  additions are names, not secrets) but not the samples. The permission table test
  enumerates the new pair. `samples.export` and `samples.clear` are audited.
- **No compatibility.** The schema is edited in place (version 4 to 5).

## Defaults

| Setting | Default |
|---|---|
| `[sampling] enabled` | true |
| `queue_depth`, `batch_size`, `batch_interval` | 1024, 50, 250 ms |
| `body_budget_bytes` | 16 MiB |
| Sampling enabled per route | inherited from the global setting (on) |
| Samples readable by | `admin` |

## Consequences

- Settings reach other instances within the sync interval; samples do not travel that way
  (ADR 0022).
- An override can only add redaction; the built-in lists cannot be weakened from anywhere.
