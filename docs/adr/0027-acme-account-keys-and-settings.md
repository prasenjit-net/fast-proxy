# ADR 0027: the ACME account, issued keys, the per-subsystem switch and the settings

Status: proposed (2026-11-15); pending the owner's review. Settings, schema and key handling are
built in PR1; the switch's rules and the certificate write in PR2. Plan reference: M4 K3, K4,
K5, K12, K13, K17. Realizes ADR 0014.

## Decision

- **Process settings** in `[data_plane.acme]` (restart to change): `enabled` (`--acme` /
  `--no-acme`), `directory` (default **Let's Encrypt staging**; production must be written
  out), optional `email`, `allowed_domains` (required when enabled), `key_type`
  (`ecdsa-p256` default, `ecdsa-p384`; `ring` cannot generate RSA), `directory_ca` (roots for a
  private CA or Pebble), `renew_check_interval`, `propagation_wait`, `lease_ttl`. The directory
  must be `https` (or `http` on loopback). ACME on needs `[data_plane.tls] enabled`. Startup warns
  for the staging directory and when no plain listener is on port 80. External Account Binding is
  not supported yet (its keys are rejected as unknown).
- **Allowlist:** `example.com` allows exactly that name; `*.example.com` allows any name below
  it, like a host pattern. An empty list is refused when ACME is on, so a typo cannot open
  issuance to every name.
- **Per-subsystem switch** `https.acmeEnabled` (column `https_acme_enabled`), part of the HTTPS
  settings and so of `Config` (a rollback restores it). It needs `https.enabled`; it is accepted
  only while ACME is on for the process (400 otherwise) and is inert (`inactive`) if the process
  switch is later turned off. While it is on, attaching another certificate is refused; turning
  it off keeps the last ACME certificate attached.
- **Eligible names** (one rule, `eligible_names`): exact names with at least two labels, inside
  the allowlist, at most 100 (the Let's Encrypt limit). Wildcards (`wildcard_needs_dns01`),
  single labels (`single_label`), names outside the list (`not_allowed`) and names past the
  hundredth (`too_many_names`) are skipped with that reason. Host patterns can never be IP
  literals, so there is no reason for them.
- **Account:** one per directory, created on the first order (turning ACME on with no subsystem
  using it contacts nobody). The credentials blob (account URL and key) is sealed with
  `Vault::seal_secret` under the label `acme-account:<directory>` and stored in `acme_account`.
  Not `Config`; never rolled back.
- **Issued certificates:** a fresh key per order and a CSR naming exactly the ordered names. The
  CA's chain must pass `validate_issued` (the upload checks plus every ordered name covered)
  before anything is stored. One apply, actor `system`, audit `certificate.acme.issue` or
  `certificate.acme.renew`: seal the key, replace `acme-<subsystem>` in place (same id, so the
  certificate cache and the cluster pickup of ADR 0011 apply) or create it, attach it, clear the
  order's challenges, and delete the subsystem's unattached `dev-<subsystem>` certificate.
- **No compatibility kept:** the schema is edited in place (version 6).

## Consequences

- A first setup cannot spend production rate limits by accident.
- Nothing secret is stored unsealed; the account key travels like every certificate key.
