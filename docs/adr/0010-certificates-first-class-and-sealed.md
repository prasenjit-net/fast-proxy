# ADR 0010: certificates are first-class, referenced and stored sealed

Status: accepted. Plan reference: M3 F2, F5, F6, F11, F24-F28.

## Context

Certificates must be managed live from the admin UI/API, shared between
subsystems (wildcards), guaranteed to exist before the first request, and kept
safe at rest while the admin plane has no authentication (until M5).

## Decision

- **Everything lives in SQLite:** uploaded, self-signed and (M4) ACME
  certificates, and CA bundles. No certificate or key is read from a file. The
  config file holds only process settings.
- **Referenced, not embedded.** A certificate (name, chain, sealed key, SAN
  names, validity, source) exists on its own; a subsystem references it by
  `https.certificate_id`. A used certificate cannot be deleted (409).
- **Routing:** SNI -> host table -> subsystem -> certificate, reusing the host
  matcher. An unknown or missing SNI, and a host of a subsystem with HTTPS
  disabled, gets a process-wide ephemeral self-signed certificate generated in
  memory at startup and never stored; `reject_unknown_sni = true` fails the
  handshake instead.
- **Guarantee (F6).** With TLS on, a subsystem with `https.enabled` always has a
  certificate: the one the admin attached or uploaded, else a self-signed dev
  certificate (`dev-<subsystem>`, SANs from the subsystem's hosts), generated and
  attached in the same write. Startup fills gaps; host changes regenerate a dev
  certificate that stops covering the hosts; an hourly sweep regenerates one with
  under 30 days left. An uploaded certificate is never auto-replaced.
- **Validation (F11):** chain and key parse (PKCS#8, PKCS#1, SEC1), key matches
  the leaf, a usable DNS SAN, not expired (under 14 days left is a warning),
  encrypted keys rejected. On attach the certificate must cover at least one of
  the subsystem's hosts; uncovered hosts are warned about. Errors are
  field-specific and never echo PEM. An unreadable stored certificate at startup
  is skipped and reported, never fatal.
- **Sealed keys (F24, F25).** Private keys are sealed with AES-256-GCM (`ring`)
  under a 32-byte master key, random nonce per write, `(id, name)` as associated
  data. A `vault` row holds an encrypted check value so a wrong master key is
  one clear message. The master key comes from `FAST_PROXY_MASTER_KEY` (base64),
  else `master_key_file` (default `<state_dir>/master.key`), created with mode
  0600 the first time a key must be sealed. Honest limit: someone holding both
  the database and the master key can read the keys; the database alone is not
  enough.
- **History (F26).** Certificate material is not part of `Config` and never
  enters a revision. A subsystem's `https` settings and certificate reference
  are configuration and are. Material changes are summary-only revisions.
  Rollback never touches material and is refused, naming the certificate, if a
  referenced certificate or bundle was deleted.
- **API safety (F27).** Private keys are write-only. Requests carrying a key are
  accepted only from a loopback peer until M5. PEM bodies are size-capped.
- **Limits (F28):** 256 certificates, 16 CA bundles, 256 KiB of PEM each, chains
  of at most 10 certificates, names 1 to 63 characters, unique.

## Consequences

- A database copy or backup alone does not expose keys; the master key is backed
  up separately and must be identical on all cluster instances.
- `store` keeps certificate rows as opaque data and never decrypts.

## Alternatives considered

- Certificates as files: rejected by the owner; breaks live management and
  clustering.
- Keys in revisions: would copy every key up to 100 times.
- External KMS/HSM: out of scope.
