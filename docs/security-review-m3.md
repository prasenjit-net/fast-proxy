# Security review of M3 (TLS termination)

Task Q8 of [M3-PLAN.md](M3-PLAN.md): the checklist run before the release, with
how each item was checked and what was found. Date: 2026-11-10. Scope: everything
M3 added (certificate storage and the key vault, the admin API for it, the HTTPS
listener, redirect, HSTS, upstream TLS, the cluster sync). The admin plane still
has no authentication (M5); that limitation is outside this review but shapes some
findings.

## Checklist

| # | Item | Result | How it was checked |
|---|---|---|---|
| 1 | Keys are never logged | pass | Every `tracing` call in the `tls`, `config-service` and data plane TLS code was read: they log paths (never contents), counts and error kinds. Every type holding a key or the master key has a hand-written `Debug` that prints no bytes (`KeyMaterial`, `MasterKey`, `Vault`, `CertHandle`, `NewCertificate`, `CertificateRow`, `VaultCheck`); tests assert it for `KeyMaterial`, `MasterKey`, `Vault` and `CertHandle`, the rest were read. The environment value of `FAST_PROXY_MASTER_KEY` is read once and not logged |
| 2 | Keys are never served | pass | No endpoint returns a key or its ciphertext (`CertificateDto` has no such field; `tls_api_tests` checks every response, event and the metrics text for the key body, `PRIVATE KEY` and `BEGIN CERTIFICATE`). Revisions carry only certificate ids and names (`no_revision_row_contains_pem_or_key_bytes`, `a_revision_shows_the_https_settings...`). The database file and its WAL contain no PEM header, no base64 key body and no raw key bytes after uploads, replacements, renames and deletions (`the_database_file_never_contains_private_key_text_or_bytes`) |
| 3 | Errors never leak paths or PEM | pass | Field errors name the field and a fixed message; tests feed text with a marker and assert it never comes back (`validate`, API and UI). Storage and I/O detail is logged and shown to clients as "an internal error occurred". Vault errors carry no path (the I/O error kind only) |
| 4 | The placeholder certificate is never served for a host that has a usable certificate | pass | `select_certificate` returns the placeholder only for an unknown or missing SNI, a subsystem with HTTPS off, or one whose certificate is unreadable (then the request gets 421). The handshake picks the certificate once from one snapshot. End-to-end tests cover each subsystem getting its own certificate under live replacement (`tls_live_replacement.rs`) |
| 5 | HSTS only over HTTPS | pass | `hsts_is_sent_over_https_only`; the header is added only when the connection has TLS state and TLS is on |
| 6 | The redirect never builds a `Location` from anything but a matched host | **finding, fixed while writing PR8, re-verified here** | A wildcard pattern matches any suffix and the host table does not vet label characters, so `Host: evil.com@a.wild.example.com` matched `*.wild.example.com`. A redirect built from it would have been an open redirect. The target now uses `dns_host` (letters, digits, `-`, `.` only); a value that fails is never put into a `Location`. Tests: routing unit tests and `a_host_with_odd_characters_is_never_turned_into_a_location` |
| 7 | Handshake and HTTP/2 limits are in force | pass | Tests with mutations: the concurrent-handshake bound refuses extras while an established connection keeps its latency; stalled handshakes are cut off at the timeout; an oversized ClientHello and a flood of failing handshakes do no harm; the stream limit, the header-list limit, rapid reset and GOAWAY drain are tested (`tls_http2.rs`) |
| 8 | `verify = false` is visible everywhere | **finding, fixed** | F16 required a warning on every load, a UI badge and a metric label. The warning and the badge existed; the metric did not. Added `fastproxy_upstream_tls{subsystem,upstream,verify}`; a group with `verify="false"` is marked, and a group that turns TLS off disappears |
| 9 | Size and count limits are in force | pass | 256 certificates, 16 CA bundles, 256 KiB of PEM, a chain of at most 10 (store and `tls` tests), and a 600 KiB request body limit on the routes that carry key material (413 in the error envelope) |

## Other findings

- **The loopback rule is only as strong as the admin listener's isolation.** Key
  uploads are accepted only from a loopback peer, judged by the connecting IP. A
  reverse proxy on the same machine in front of the admin plane makes every request
  look local and defeats the rule. Keep the admin plane unexposed until M5 adds
  authentication (the README says so).
- **The master key is visible to anyone who can read the process environment**
  when it is supplied as `FAST_PROXY_MASTER_KEY` (for example `/proc/<pid>/environ`
  by the same user or root). The key file is created with mode 0600 on Unix and a
  warning is logged if it is group or world readable; on Windows no permission check
  is made. Both are stated as honest limits: the design protects a copy of the
  database or its backups, not a host an attacker already controls.
- **Decrypted keys live in memory** inside rustls structures; the buffers this code
  owns are zeroed, but rustls does not zero its own. Core dumps and swap can expose
  them (F8's documented limit).
- **A development certificate is trusted by no one** and says `fast-proxy.invalid`
  in the placeholder case; this is intended and labelled (source badge, dashboard
  and certificates-page warnings).
- **CA bundle uploads are not restricted to loopback**: they are public material.
- **Residual: no admin authentication until M5**, so anyone who can reach the admin
  plane can replace a certificate with one whose key they hold. Nothing in M3 makes
  that worse than the existing ability to reroute any host.

## Result

Two real defects were found (the redirect host, caught while writing PR8, and the
missing metric, caught by this review) and fixed with tests; the rest of the
checklist held. No open issue
blocks the release.
