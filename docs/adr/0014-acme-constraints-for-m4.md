# ADR 0014: ACME constraints recorded for M4

Status: accepted (design constraints only; nothing is built in M3). Plan
reference: M3 F29.

## Decision

- ACME settings stay in the config file: `[data_plane.acme]` (`enabled`,
  `email`, `directory`, optional allowlist; `--acme` flag).
- Per-subsystem ACME (`https.acme_enabled`, a column added in M4) is accepted
  only when ACME is enabled for the process (a 400 on `acme.enabled` otherwise)
  and is inert if the process switch is later turned off.
- ACME issues for the subsystem's non-wildcard hosts and writes the certificate
  through the same path as any other (`source = acme`, attached to the
  subsystem, replaced in place on renewal), so the cache recycle and the cluster
  pickup of ADR 0011 apply unchanged.
- A subsystem uses either an ACME-managed or a manually attached certificate:
  turning ACME on replaces the attachment; turning it off keeps the last
  certificate until it expires.
- In a cluster only one instance may renew (a lease row taken by
  compare-and-swap in a transaction), and HTTP-01 challenge tokens are stored in
  the database so any instance can answer the challenge.
- The ACME account key is sealed by the same vault (confirmed in M4).
