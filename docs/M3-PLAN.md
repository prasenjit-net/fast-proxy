# M3 Plan: TLS termination

Status: **v0.4 (2026-11-10), in progress.** The **(confirm)** items were accepted as
recommended when the owner asked to start implementing (recorded in ADRs 0009-0014).
v0.3 followed three decisions by the
owner: (1) **all certificates live in SQLite** (uploaded, auto-generated
development certificates, and later ACME-issued ones); (2) **TLS is switched on
for the whole process** by configuration or a command-line flag, which enables
the HTTPS listener, and **each subsystem then opts in** to HTTPS, configures
HSTS, and gets a certificate (pre-uploaded, or an auto-generated dev
certificate); (3) **ACME settings stay in the config file** (M4), the issued
certificates go to SQLite, and ACME is enabled per subsystem only when it is
enabled for the process. It also designs for **several instances running as a
cluster** and a later move from SQLite to PostgreSQL: certificates are cached in
memory and recycled when they change, including when another instance changed
them.

Items marked **(confirm)** need the owner's sign-off before the tasks that depend
on them start. Parent document: [ROADMAP.md](ROADMAP.md) (section 4.3 and the M3
list). Predecessor: [M2-PLAN.md](M2-PLAN.md) (complete; section 11 there lists
what carries over). Task sizes are relative effort (S under half a day, M about
1-2 days, L about 3-5 days), not commitments. Tick the checkboxes as tasks land.

## 1. Goal and exit criteria

Until now the proxy speaks plain HTTP/1.1 to clients and to upstreams. M3 lets
it **terminate TLS**: one switch turns on an HTTPS listener that picks the
certificate of the subsystem a hostname belongs to, negotiates HTTP/2 or
HTTP/1.1, and takes certificates **live from the admin UI/API** (stored
encrypted in the database and cached in memory). Each subsystem decides whether
it is served over HTTPS, redirects plain HTTP, and sends HSTS. The proxy can
also **talk TLS to upstreams** (re-encrypt), including for health checks. ACME
(M4) writes into the certificate store built here.

**Exit criteria** (from the roadmap, made measurable):

1. **TLS is a process-wide switch.** With it off (the default) there is no TLS
   listener and no TLS cost. With it on (`[data_plane.tls] enabled = true` or
   `--tls`; the flag wins) the HTTPS listener starts next to the plain ones.
2. **Each subsystem opts in.** Only hosts of subsystems with HTTPS enabled are
   served over HTTPS, with **that subsystem's certificate** chosen by SNI (a
   wildcard certificate may be shared by several subsystems), over **both
   HTTP/1.1 and HTTP/2** negotiated by ALPN on the same port. A host of a
   subsystem with HTTPS disabled gets **421** on the HTTPS listener.
3. **A certificate is guaranteed before the first request.** Enabling HTTPS on a
   subsystem attaches an uploaded certificate if the admin supplied one (before
   or at that moment), otherwise it **generates a self-signed development
   certificate**, stores it and attaches it, in the same write. Startup fills any
   gap, and the dev certificate follows host changes.
4. **Certificates are managed live and recycled everywhere.** Uploading,
   replacing or deleting a certificate changes what the next handshake gets, with
   no restart and without dropping an established connection; an invalid upload
   (unparseable, a key that does not match, expired, an encrypted key, a
   certificate that covers none of the subsystem's hosts) is rejected with a
   field-specific error and changes nothing. **Another instance sharing the
   database picks the change up within a few seconds** (demonstrated with two
   instances on one database).
5. **Private keys are protected.** Stored **encrypted** (the database file alone
   does not contain them), **never returned** by the API or written to a log,
   metric or error, never in revision history, and accepted only from a loopback
   peer until admin authentication exists (M5).
6. A **TLS policy** in the config file is enforced: a client offering only
   versions below the configured minimum cannot connect.
7. Per subsystem, **plain HTTP can redirect to HTTPS** and **HSTS can be turned
   on**, live from the admin UI/API; both need HTTPS enabled on the subsystem
   and take effect only while TLS is on for the process.
8. A request whose `Host` is served by a **different certificate** than the one
   its SNI selected gets **421 Misdirected Request**.
9. An upstream group can be marked **TLS** (verification on by default, an
   optional server-name override, and a CA bundle **uploaded** and chosen by
   name), traffic and health checks to it use TLS, and a certificate the proxy
   does not trust is refused.
10. It is all **observable**: handshake counts by result and version, certificate
    expiry per certificate, cache and sync metrics, the protocol of each request
    in the access log, and a certificate card on each subsystem, a certificates
    page and a dashboard warning in the admin UI.
11. **No regression:** the M1 and M2 exit criteria and load tests still pass over
    plain HTTP with TLS off, and plain-HTTP throughput stays within the M2
    baseline (section 8).

## 2. Scope

### In scope

- `[data_plane.tls]` in the config file: the on/off switch (and the `--tls` /
  `--no-tls` / `--https-listen` flags), the HTTPS listen address, the TLS policy
  (minimum version, HTTP/2, handshake limits), the master-key location and the
  sync interval. **No certificate or key file is read from disk.**
- Certificate storage in SQLite: server certificates (private keys encrypted at
  rest) and CA bundles; a key vault; live upload, replace, delete and
  self-signed generation through the admin API.
- Per-subsystem HTTPS settings (enabled, certificate, HSTS, redirect) and the
  certificate guarantee (auto-generated dev certificates).
- An in-memory certificate cache carried atomically with the routing snapshot,
  and a version-poll sync so a change made through another instance is picked up.
- A TLS crate: PEM parsing and validation, the key vault, the self-signed
  generator, the cache, rustls server and client configurations.
- HTTP/2 to clients (ALPN `h2`, `http/1.1`) with explicit limits and a graceful
  GOAWAY drain; SNI-versus-`Host` cross-check (421); scheme-aware
  `X-Forwarded-Proto` and `Forwarded`.
- Upstream TLS per group (verify, server-name override, named uploaded CA bundle),
  also for health probes.
- Admin API and UI for all of it; metrics and events.
- The M2 carry-over items in workstream C.

### Out of scope (later milestones, or never)

| Item | Where |
|---|---|
| ACME: the client, HTTP-01, renewal, and the per-subsystem ACME switch. Its **settings** (`[data_plane.acme]`: enabled, email, directory, allowlist) stay in `config.toml`; issued certificates are stored here with `source = acme` (design constraints in F29) | M4 |
| A **PostgreSQL** backend and running several instances against one network database. M3 makes the certificate path ready (F9) but ships SQLite only; sharing one SQLite file between instances over a network filesystem is unsupported | later project (after M4) |
| Cluster-wide health state, shared TLS session-ticket keys, leader election for background jobs | later; instance-local in M3 (F9) |
| Admin authentication, TLS on the admin listener | M5 (until then key upload is loopback-only, F27) |
| Maintenance mode | M5 |
| Rate and connection limits per client | M7 |
| OCSP stapling, certificate transparency, client-certificate (mTLS) authentication of clients | after 1.0 |
| HTTP/2 **to upstreams** (upstream connections stay HTTP/1.1 over TLS), HTTP/3 | revisit after M3 **(confirm, F3)** |
| WebSocket over HTTP/2 (RFC 8441); clients use HTTP/1.1 for upgrades, which ALPN already arranges | not planned |
| Passphrase-protected private keys on upload; external KMS or HSM for the master key | not planned: upload an unencrypted key, the proxy encrypts it at rest |
| Reading certificates from files or watching files | dropped (everything lives in SQLite) |
| TLS 0-RTT (early data) | never: replay risk |

## 3. Design decisions for M3

Items marked **(confirm)** are recommendations that need your sign-off.

| # | Decision | Recommendation |
|---|---|---|
| F1 | Crypto provider | `rustls` with the **`ring`** provider (`tokio-rustls`, `hyper-rustls` for the upstream side); `ring` also provides the AES-GCM of F24. `aws-lc-rs` needs a C toolchain (cmake, sometimes NASM on Windows) and CI runs on Windows, so `ring` is the lower-risk choice; the provider is one line to swap later. **(confirm)** |
| F2 | Where things live | **Config file:** the process-wide TLS switch and listen address, the TLS policy, the master-key location, the sync interval, and (M4) the ACME settings. **SQLite:** all certificates and keys (uploaded, self-signed, later ACME), CA bundles, each subsystem's HTTPS settings and certificate reference, and (M4) the ACME account key sealed by the vault **(confirm in M4)**. No certificate or key is read from a file. (Decided by the owner.) |
| F3 | HTTP/2 scope | HTTP/2 **to clients** (ALPN) in M3. Upstream connections stay HTTP/1.1 (over TLS when the group says so): the pool, retry and body-replay code is built for HTTP/1.1, and h2 upstream multiplexing changes in-flight accounting. Revisit with data (C4). **(confirm)** |
| F4 | Enablement model | **Process level:** `[data_plane.tls] enabled` (default `false`), overridden by `--tls` / `--no-tls`. When on, exactly one **HTTPS listener** starts at `listen` (`--https-listen`; default `0.0.0.0:8443`, use 443 in production) alongside the existing plain `[[data_plane.listener]]` ones, which keep serving. When off there is no TLS listener, no handshakes and no TLS cost; subsystem HTTPS settings stay stored but are **inert**, and the API and UI say so (`effective: false`). **Subsystem level:** `https.enabled` (default `false`) says whether the subsystem is served over the HTTPS listener; certificate, redirect and HSTS are configured under it. Per-listener `tls = true` is dropped. |
| F5 | Certificates are first-class and referenced | A certificate (name, chain, sealed key, SAN names, validity, source) exists on its own and a subsystem **references** it (`https.certificate_id`); several subsystems may share one (a wildcard certificate is stored once, a renewal updates all of them). TLS routing is **SNI to host table to subsystem to its certificate**, reusing the host matcher (exact name, then the longest wildcard). An **unknown or missing SNI**, and a host of a subsystem with HTTPS disabled, gets a **process-wide ephemeral self-signed "unknown host" certificate** generated in memory at startup and never stored, so the handshake succeeds and the HTTP layer answers 404 or 421; `reject_unknown_sni = true` fails the handshake instead. A certificate cannot be deleted while a subsystem uses it (409). **(confirm)** |
| F6 | The certificate guarantee and dev certificates | When TLS is on, a subsystem with `https.enabled` **always has a certificate before its first request**. Enabling HTTPS attaches the certificate the admin chose or uploaded in that request, else **generates a self-signed development certificate** (`rcgen`, the subsystem's host names and wildcard patterns as SANs, named `dev-<subsystem>`, `source = self_signed`), stores and attaches it **in the same write**. Startup fills any gap. Adding or removing a host regenerates a dev certificate that no longer covers the hosts, in the same write. A dev certificate with under 30 days left is regenerated by an hourly sweep. An **uploaded certificate is never auto-replaced**. Detaching a certificate while HTTPS is enabled falls back to a dev certificate. With TLS off, nothing is generated (no listener) but uploads are still accepted ("pre-upload"). **(confirm)** |
| F7 | Certificates travel with the routing snapshot | The compile step takes the certificate handles (by id) and the process TLS state as inputs, so each `CompiledSubsystem` carries its `https` settings **and its certificate handle**; the handshake resolver loads the snapshot, finds the subsystem by SNI and returns that handle. **One atomic swap changes routing, HTTPS settings and certificates together**: a request or handshake never sees a new setting with an old certificate, and the 421 rule (F14) compares certificate ids from the same snapshot. The compile veto also rejects HTTPS enabled with no usable certificate (a bug guard: the reconciler of F6 prevents it). |
| F8 | In-memory certificate cache | A `CertCache` holds the **parsed, decrypted** `rustls` certified keys, keyed by certificate id and a version (`updated_at` and fingerprint). Every rebuild **reuses unchanged entries, parses only new or changed rows and drops removed ones**, so a renewal costs one parse, not N. Replaced handles stay alive through `Arc` until handshakes using them finish; established connections are untouched. Decrypted keys exist only in this cache (inside rustls structures, zeroed where the crates allow) and are never written anywhere. A row that cannot be read (corruption, a wrong master key) is marked **unreadable** and reported; its subsystems get the ephemeral certificate and 421 until it is fixed. |
| F9 | Cluster readiness | Designed for several instances and a later PostgreSQL, **built for one instance in M3**: (a) every committed change, configuration or certificate (certificate changes are summary-only revisions, F26), bumps `revision.version`, so **the revision version is the cluster generation**; (b) each instance runs a **sync poll** (`config_poll_interval`, default 5 s, 0 disables) comparing the database version with its live snapshot's; when it advanced (another instance or process wrote), it reloads configuration and certificates from the database and swaps, reusing runtime state and the certificate cache; the writing instance sees its own change at once, others within the interval (eventual consistency, stated in the docs); (c) certificate reads sit behind a narrow `CertificateSource` trait and the certificate SQL is portable (`BLOB` to `BYTEA`, millisecond `BIGINT` times, JSON as text, no SQLite-only features), so PostgreSQL can implement it later, while writes still go through `ConfigService`, whose own storage abstraction is part of that later project; (d) **instance-local by design:** endpoint health and ejection, in-flight counts, TLS session-ticket keys (resumption works within one instance); (e) the **master key must be identical on all instances** (environment variable or a secret mount); the auto-created key file is a single-instance convenience, and a mismatch is detected by the vault check and reported; (f) ACME constraints for a cluster are fixed now in F29. **(confirm)** |
| F10 | One write path | A certificate or HTTPS write goes through **`ConfigService`**, like every other write: take the writer lock, run the change in a transaction (seal the key, write rows), **reconcile dev certificates** (F6), **veto by compiling the would-be snapshot with the would-be certificates**, commit with a revision (summary-only for pure certificate changes), swap the snapshot, broadcast. There is no separate reload mechanism, file watcher or "reload now" endpoint: a write is live, and a write by another instance arrives through the sync poll (F9). |
| F11 | Validation | On **upload or replace**: the chain and key parse (PEM; PKCS#8, PKCS#1, SEC1), the key matches the leaf, the leaf has a usable DNS SAN, it is **not expired** (expiring within 14 days is accepted with a warning), an **encrypted** key is rejected with a clear message. On **attach** to a subsystem: the certificate must **cover at least one** of the subsystem's host patterns (otherwise rejected: it is almost certainly the wrong certificate), and hosts it does not cover are **warned about** in the response and shown in the UI. `covers(san, pattern)`: equal names; or a wildcard SAN `*.d` covering an exact host with exactly one more label under `d`; a wildcard host pattern `*.d` is covered only by the same wildcard SAN. Errors are field-specific (`certPem`, `keyPem`, `name`, `https.certificateId`) and never echo PEM. At **startup** an unreadable stored certificate is skipped and reported, never a reason to refuse to start. |
| F12 | TLS policy | `min_version = "1.2"` (default) or `"1.3"`, process-wide, in the config file. rustls's default cipher suites (no knob in M3). ALPN `h2`, `http/1.1`; `http2 = false` turns h2 off. Session tickets **on** (rustls `Ticketer`, rotated), early data **off**. |
| F13 | Redirect and HSTS | Per subsystem, under `https`: `redirect` (`off`, `308` default when on, or `301`) and HSTS (`enabled`, `max_age_secs`, `include_subdomains`; no `preload` in M3). **Both are only accepted when `https.enabled` is true** (a 400 names the field otherwise) and are **effective only while TLS is on for the process** (a redirect to a listener that does not exist would be a trap). The redirect applies to requests that arrived on a **plain-HTTP listener**, after the host matched a subsystem and before routing; the target is `https://<request host>[:port]<path and query>`, the port being the HTTPS listener's `public_port` (default: its listen port; omitted when 443). HSTS is only ever sent on HTTPS responses (RFC 6797). The ACME challenge path will bypass the redirect in M4, so the check lives in one function. **(confirm)** |
| F14 | SNI versus Host (421) | On the HTTPS listener: (1) a request for a host of a subsystem with **HTTPS disabled** gets 421; (2) a request whose host is served by a **different certificate** than the one SNI selected gets 421 (hosts sharing one certificate pass, which keeps browsers' HTTP/2 connection coalescing working); (3) no SNI (a client connecting by IP) skips rule 2. **(confirm)** |
| F15 | Upstream TLS settings | Per group, in the database: `tls` (off by default), `verify` (on by default), `server_name` (override for SNI and the certificate check; needed in practice when the endpoint is an IP), and `ca` (the **name of an uploaded CA bundle**; empty means the OS trust store via `rustls-native-certs`). CA bundles are public material in their own table, referenced **by name**; the compile veto rejects an unknown name, also on rollback. |
| F16 | Skipping verification | `verify = false` is allowed (self-signed internal services are common) but logged as a warning on every config load that contains it, shown with a warning badge in the UI, and exposed as a label on the upstream-TLS metric. **(confirm)** |
| F17 | Health checks over TLS | An HTTP probe to a TLS group **uses TLS with the group's settings**, so it tests the path real traffic takes; the TCP probe stays a plain connect. No new health kind. |
| F18 | Scheme awareness | The connection context carries the scheme, the TLS version, the ALPN protocol, the SNI and the certificate id. `X-Forwarded-Proto` and `Forwarded` use the real scheme instead of the fixed `"http"` of M1, and the upstream URI's scheme follows the group's `tls` flag. |
| F19 | Handshake safety | Handshake timeout (10 s) and a bound on concurrent handshakes (default 256) so a handshake flood cannot starve established traffic; failures are counted by reason, never logged one line each at info level. |
| F20 | HTTP/2 hardening | Explicit limits (max concurrent streams 128, header list size, initial windows, keep-alive ping with timeout), the `h2` rapid-reset mitigations on, and GOAWAY on graceful shutdown so in-flight streams finish. Config-file knobs with safe defaults. |
| F21 | Crates | New crate `fast-proxy-tls`: PEM parsing and validation, the key vault, the self-signed generator, `CertHandle` and `CertCache`, rustls `ServerConfig`/`ClientConfig` builders, certificate status. It depends on `model` and `telemetry`. **`routing` depends on `tls`** for the handle type (data only, no I/O), so the snapshot can carry certificates (F7). The **SNI resolver lives in the data plane** (it needs the snapshot). `store` keeps certificate rows as **opaque** data (ciphertext and metadata) and gains no crypto dependency; `config-service` depends on `tls` (to seal, parse, reconcile and compile). Dependencies still point one way. **(confirm)** |
| F22 | Metric labels | Certificate labels come from the certificate's **name** (admin-chosen, unique, at most 256 certificates, so cardinality is bounded by configuration), never from handshake data. Handshake metrics are labelled by listener, result and TLS version only. Series of deleted certificates are deleted. |
| F23 | Compatibility | None kept: the project is undeployed. The schema is edited in place (version 2 to 3). |
| F24 | Keys encrypted at rest | Private keys are sealed with **AES-256-GCM** (`ring`) under a 32-byte **master key** before they reach the database; the nonce is random per write and the certificate id and name are bound as associated data, so a ciphertext cannot be moved to another row. A `vault` row holds an encrypted check value, so a wrong master key is detected once at startup with a clear message rather than as N failed certificates. Honest limit: anyone who can read **both** the database and the master key can read the keys; the point is that a database copy, backup or leak on its own does not expose them. **(confirm)** |
| F25 | Master key handling | From the environment variable `FAST_PROXY_MASTER_KEY` (base64) if set, else the file `data_plane.master_key_file` (default `<state_dir>/master.key`), **created with random bytes the first time a key must be sealed** (mode 0600 on Unix, a warning if it is group or world readable). With TLS off and no certificate stored, nothing is created. Losing the key means stored private keys cannot be read: they are reported and certificates must be uploaded again (dev certificates regenerate). The docs say to back the file up separately from the database, and that **all cluster instances must share the same key** (F9). **(confirm)** |
| F26 | Certificates and history | Certificate **material** (chains, keys, bundles) is **not part of `Config` and never enters a revision**: revisions copy the whole configuration, which would copy every key up to 100 times. A subsystem's `https` settings **and its `certificate_id`** are configuration and are in revisions. A change to certificate material is recorded as a **summary-only revision** (the configuration unchanged: "certificate uploaded: main"), which also bumps the cluster generation (F9). **Rolling back never touches certificate material.** A rollback to a revision that references a certificate or CA bundle that has since been deleted is refused with a clear message naming it (upload it again first); revisions therefore also record the certificate's **name** for that message. |
| F27 | Admin API safety | Private keys are **write-only**: no endpoint ever returns one (or the ciphertext). Requests that **carry a private key** (upload, replace, self-signed, upload-and-attach) are accepted **only from a loopback peer**, and refused with 403 and an explanation otherwise, until admin authentication exists (M5). PEM bodies are size-capped; errors, logs and metrics never contain PEM content or key bytes. Reads return metadata only (name, source, SAN names, subject, issuer, validity, fingerprint, which subsystems use it). **(confirm)** |
| F28 | Limits | At most 256 certificates and 16 CA bundles; each chain or bundle at most 256 KiB of PEM, a chain at most 10 certificates; names unique, 1 to 63 characters. These bound memory, compile cost and metric cardinality. |
| F29 | ACME readiness (design constraints for M4, nothing built in M3) | `[data_plane.acme]` in the config file (`enabled`, `email`, `directory`, an optional allowlist; `--acme` flag). **Per-subsystem ACME** (`https.acme_enabled`, a column added in M4) is accepted **only when ACME is enabled for the process** (a 400 `acme.enabled` otherwise) and is inert if the process switch is later turned off. ACME issues for the subsystem's non-wildcard hosts and **writes the certificate through the same path** (`source = acme`, attached to the subsystem, replaced in place on renewal, so F8's recycle and F9's cluster pickup apply unchanged). A subsystem uses either an ACME-managed or a manually attached certificate: turning ACME on replaces the attachment, turning it off keeps the last certificate until it expires. **In a cluster, only one instance may renew** (a lease row taken by compare-and-swap in a transaction), and **HTTP-01 challenge tokens are stored in the database** so whichever instance receives the challenge request can answer it. |

## 4. Target architecture

```
        config.toml / CLI                          SQLite (all live settings)
 [data_plane.tls] enabled, listen, policy            subsystem: https {enabled, certificate_id, redirect, HSTS}
 master_key_file / FAST_PROXY_MASTER_KEY             upstream group: tls, verify, server_name, ca
 config_poll_interval                                certificate: chain, key (ENCRYPTED), SANs, validity, source
 (M4: [data_plane.acme] settings)                    ca_bundle   vault (key check)   revision (= cluster generation)
              |                                                     |
              v                                                     v
        fast-proxy-tls                         ConfigService (the one writer)
   KeyVault (AES-GCM, master key)               lock -> transaction -> seal keys -> reconcile dev certs
   CertCache (parsed keys by id+version)        -> veto: compile snapshot with the would-be certificates
   self-signed generator, PEM validation        -> commit (summary-only revision for cert changes) -> swap
        |                                              ^                                  |
        |                           sync poll: db version > snapshot version?             v
        |                           (another instance wrote) -> reload, swap      Snapshot (atomic):
        v                                                                        hosts -> subsystem { routes, upstreams,
  data plane                                                                       https {enabled, redirect, hsts, cert handle} }
   accept -> [TLS handshake (timeout, bound)] -> hyper auto (h1 | h2)  <-- resolver: SNI -> snapshot -> subsystem cert
   ConnectionContext { scheme, tls version, alpn, sni, cert id }           (unknown/no SNI: ephemeral in-memory cert)
   request: host -> subsystem -> [redirect/HSTS] -> 421 rules -> route -> select endpoint
        v
  UpstreamClient: http or TLS connector per (connect timeout, tls settings)
        |   ClientConfig: OS roots or an uploaded CA bundle, verify on/off, SNI override
        v
  your backends (plain or TLS)         health probes use the same TLS settings
```

Crate dependencies (one way, no cycles):

```
model <- tls <- routing <- dataplane
model <- balancing <- routing
model <- store                       (certificate rows are opaque blobs here)
tls + routing + store + balancing + telemetry <- config-service
tls + routing + balancing + telemetry <- health
config-service + tls + health <- admin
everything <- the binary
```

- **`fast-proxy-model`** (pure) gains `HttpsPolicy` on the subsystem (enabled,
  redirect, HSTS, certificate reference), `UpstreamTls` on the group, the
  certificate metadata types and limits, and the `covers` rule.
- **`fast-proxy-tls`** (new) owns everything about certificates, keys and rustls
  configurations; no HTTP knowledge.
- **`fast-proxy-store`** gains the `certificate`, `ca_bundle` and `vault` tables and
  their repository functions, storing ciphertext without ever decrypting it, and
  the `CertificateSource` read trait.
- **`fast-proxy-routing`** compiles the new settings and certificates into the
  snapshot.
- **`fast-proxy-config-service`** applies certificate writes under the same writer
  lock as every other write, reconciles dev certificates, and runs the sync poll.
- **`fast-proxy-dataplane`** does the handshake, resolves certificates from the
  snapshot, serves h2, applies the redirect, HSTS and 421, and connects to TLS
  upstreams.
- **`fast-proxy-health`** probes TLS groups with TLS.
- **`admin`** / **UI** carry the settings and the certificate screens.

### 4.1 One HTTPS request, in order

1. The HTTPS listener accepts a TCP connection; a handshake permit is taken (F19).
2. The handshake runs under a timeout. The resolver loads the snapshot, finds the
   subsystem whose host matches the SNI, and returns **its** certificate handle
   (or the ephemeral one for an unknown or missing SNI, F5). Failures are counted
   by reason; success records the version, ALPN protocol and the certificate id.
3. hyper's auto builder serves HTTP/2 when ALPN said `h2`, else HTTP/1.1. The
   connection context carries scheme, TLS version, ALPN, SNI and certificate id.
4. For each request: load the snapshot once, match host to subsystem as in M1.
5. **421 rules (F14):** the subsystem has HTTPS disabled, or its certificate id
   differs from the connection's: answer 421.
6. Route, select an endpoint and forward as in M2, with `X-Forwarded-Proto: https`
   and the upstream URI scheme and TLS settings of the chosen group.
7. On the way out, add `Strict-Transport-Security` if the subsystem enables it.
8. The access log line gains the protocol and TLS version; metrics are updated.

For a request on a **plain-HTTP listener**, step 5 is replaced by the redirect
check (F13): if TLS is on, the subsystem has HTTPS enabled and a redirect set,
answer 308 with `Location` before routing.

### 4.2 Certificate lifecycle

```
start:     open the database; TLS on or any certificate stored -> unlock the vault
           (master key from env or file; the file is created the first time a key must be sealed)
           vault check fails -> report "master key does not match this database"; stored keys unreadable
           parse and decrypt every certificate into the CertCache; unreadable ones are marked, never fatal
           TLS on: every subsystem with https.enabled and no usable certificate gets a dev certificate (F6)
           build the snapshot (certificates inside) and the ephemeral unknown-host certificate; start listeners
enable HTTPS on a subsystem / upload / replace / attach / detach / delete / generate / edit hosts (admin API):
           ConfigService: writer lock -> transaction -> seal and store keys -> reconcile dev certificates
           -> veto: compile the would-be snapshot with the would-be certificates (fails -> roll back, name the field)
           -> commit (revision; summary-only for certificate material) -> swap -> broadcast certState
           new handshakes see it at once; established connections keep what they negotiated
hourly:    refresh expiry gauges; activity entry and UI warning below 14 days;
           regenerate a dev certificate under 30 days left (a normal write)
```

### 4.3 In a cluster (design, M3 builds the single-instance path)

```
instance A: admin writes (renewal, upload, setting)  -> commit: revision version 41 -> swaps its snapshot
instance B: sync poll every 5 s: database version 41 > my snapshot version 40
            -> load configuration and certificate rows -> CertCache reuses unchanged entries, parses the changed one
            -> compile (runtime state of endpoints carried over, as in M2) -> swap
            new handshakes on B see the renewed certificate; B's established connections are untouched
instance-local: endpoint health and ejection, in-flight counts, session-ticket keys
shared:         the database, and the master key (the same on every instance)
```

Instances can be a few seconds apart after a write (eventual consistency). A
renewal is safe in that window: both the old and the new certificate are valid.

## 5. Configuration and data model changes

### 5.1 Config file (`config.toml`) and command line

```toml
[data_plane]
master_key_file = "./state/master.key"   # optional; FAST_PROXY_MASTER_KEY overrides
config_poll_interval = "5s"              # optional; "0s" turns the cluster sync off

[data_plane.tls]
enabled = false            # the process-wide switch; --tls / --no-tls override it
listen = "0.0.0.0:8443"    # the HTTPS listener (--https-listen); use 0.0.0.0:443 in production
public_port = 443          # optional: the port redirects point at (NAT, containers); default = listen's
min_version = "1.2"        # "1.2" (default) or "1.3"
http2 = true               # optional, default true
reject_unknown_sni = false # optional: fail the handshake for an unknown name instead of the placeholder certificate
handshake_timeout = "10s"
max_concurrent_handshakes = 256

# M4 (not part of M3):
# [data_plane.acme]
# enabled = false           # --acme overrides; subsystems can only enable ACME when this is on
# email = "ops@example.com"
# directory = "https://acme-staging-v02.api.letsencrypt.org/directory"
```

There are no `cert` or CA entries and no per-listener `tls` key: certificates and
CA bundles are managed in the admin UI/API and live in the database. Rules
enforced at startup: `listen` does not clash with a plain listener or the admin
address; `public_port` and the limits are in range; the master key file is
readable, or creatable when absent; unknown keys are rejected as before. Turning
`enabled` on with no certificates is fine: dev certificates are generated for
subsystems that enable HTTPS.

### 5.2 Database (schema 2 to 3, edited in place; no migration)

```sql
ALTER subsystem      -- shown as additions; edit CREATE TABLE directly
  https_enabled         INTEGER NOT NULL DEFAULT 0,
  https_certificate_id  INTEGER REFERENCES certificate(id),   -- NO ACTION: a used certificate cannot be deleted
  https_redirect        TEXT    NOT NULL DEFAULT 'off',       -- off | 301 | 308
  hsts_enabled          INTEGER NOT NULL DEFAULT 0,
  hsts_max_age_secs     INTEGER NOT NULL DEFAULT 31536000,
  hsts_include_subdomains INTEGER NOT NULL DEFAULT 0

ALTER upstream_group
  tls_enabled           INTEGER NOT NULL DEFAULT 0,
  tls_verify            INTEGER NOT NULL DEFAULT 1,
  tls_server_name       TEXT    NOT NULL DEFAULT '',          -- '' = use the endpoint's host
  tls_ca                TEXT    NOT NULL DEFAULT ''           -- '' = the OS trust store; else a ca_bundle name

CREATE TABLE certificate (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,           -- the label in metrics and status
  source      TEXT NOT NULL,                  -- uploaded | self_signed | acme (M4)
  chain_pem   TEXT NOT NULL,                  -- public: leaf first, then the chain
  key_enc     BLOB NOT NULL,                  -- AES-256-GCM ciphertext of the private key
  key_nonce   BLOB NOT NULL,
  names       TEXT NOT NULL,                  -- JSON array: DNS names and wildcards from the SANs
  subject     TEXT NOT NULL,
  issuer      TEXT NOT NULL,
  not_before  INTEGER NOT NULL,               -- ms since the epoch
  not_after   INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,                  -- SHA-256 of the leaf, hex
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL                -- with the fingerprint, the cache version (F8)
);

CREATE TABLE ca_bundle (
  id          INTEGER PRIMARY KEY,
  name        TEXT NOT NULL UNIQUE,
  pem         TEXT NOT NULL,                  -- public
  cert_count  INTEGER NOT NULL,
  created_at  INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE TABLE vault (                           -- one row: proves the master key is the right one
  id          INTEGER PRIMARY KEY CHECK (id = 1),
  check_nonce BLOB NOT NULL,
  check_blob  BLOB NOT NULL
);
```

Rules (code, and tests): `https_redirect` is one of the three values; redirect and
HSTS settings other than the defaults require `https_enabled`; HSTS
`max_age_secs` is 0 to 63,072,000 (two years); a certificate reference must exist
and cover at least one of the subsystem's hosts (F11); `tls_server_name` is empty
or a valid DNS name; `tls_ca` is empty or names a stored bundle (the compile
veto); the limits of F28; the associated data of a sealed key is `(id, name)`.
The certificate SQL avoids SQLite-only features so a PostgreSQL implementation
needs type changes only (F9).

### 5.3 API shape (sketch)

```json
// subsystem (GET /api/subsystems/{id}, PUT body): a new "https" object
"https": {
  "enabled": true,
  "certificateId": 7,
  "redirect": "308",
  "hsts": { "enabled": true, "maxAgeSecs": 31536000, "includeSubdomains": false },
  "effective": true,                      // read-only: false while TLS is off for the process
  "certificate": { "name": "shop", "source": "uploaded", "notAfter": 1790000000000,
                   "uncoveredHosts": ["old.shop.example.com"] }   // read-only
}

// upstream group: a new "tls" object
"tls": { "enabled": true, "verify": true, "serverName": "backend.internal", "ca": "internal" }
```

Setting `https.enabled` with no `certificateId` generates and attaches a dev
certificate in the same write (F6). Certificates (metadata only on reads; the key
is write-only):

```
GET    /api/certificates             list: id, name, source, names, subject, issuer, notBefore, notAfter,
                                     secondsLeft, state (ok|expiring|expired|unreadable), fingerprint,
                                     usedBy [subsystem ids], updatedAt
POST   /api/certificates             { name, certPem, keyPem }                  loopback only (F27)
PUT    /api/certificates/{id}        { name?, certPem?, keyPem? }               replace material and/or rename
DELETE /api/certificates/{id}        409 while a subsystem uses it
POST   /api/certificates/self-signed { name, names: [..] }                      loopback only
POST   /api/subsystems/{id}/certificate   { name, certPem, keyPem }             upload and attach in one step (loopback only)
DELETE /api/subsystems/{id}/certificate   detach (a dev certificate takes over while HTTPS is enabled)
GET    /api/ca-bundles               list: id, name, certCount, subjects, notAfter
POST   /api/ca-bundles               { name, pem }
DELETE /api/ca-bundles/{id}          409 while an upstream names it
GET    /api/tls                      process state: enabled, listen, http2, minVersion, vault (ok | key mismatch | not created),
                                     sync (interval, last seen version), and (M4) acme { enabled }
```

The WebSocket gains a `certState` event (changed, expiring, unreadable).

### 5.4 Defaults (G3)

The single source for validation and the UI.

| Setting | Default | Bounds / values |
|---|---|---|
| `data_plane.tls.enabled` | `false` | bool; `--tls` / `--no-tls` override |
| `data_plane.tls.listen` | `0.0.0.0:8443` | not equal to a plain listener or the admin address |
| `data_plane.tls.public_port` | HTTPS listen port | 1-65535; omitted from redirects at 443 |
| `data_plane.tls.min_version` | `"1.2"` | `"1.2"`, `"1.3"` |
| `data_plane.tls.http2` | `true` | bool |
| `data_plane.tls.reject_unknown_sni` | `false` | bool |
| `data_plane.tls.handshake_timeout` | `10s` | above zero |
| `data_plane.tls.max_concurrent_handshakes` | `256` | at least 1 |
| `data_plane.master_key_file` | `<state_dir>/master.key` | readable, or creatable |
| `data_plane.config_poll_interval` | `5s` | `0s` disables |
| `https.enabled` | `false` | bool |
| `https.redirect` | `off` | `off`, `301`, `308` (`308` when switched on without a value); needs `https.enabled` |
| `https.hsts.enabled` | `false` | needs `https.enabled` |
| `https.hsts.maxAgeSecs` | `31536000` | 0-63,072,000 |
| `https.hsts.includeSubdomains` | `false` | bool |
| `tls.enabled` (upstream) | `false` | bool |
| `tls.verify` | `true` | bool |
| `tls.serverName` | empty (endpoint host) | empty or a valid DNS name |
| `tls.ca` | empty (OS trust store) | empty or an uploaded bundle name |
| Certificates | at most 256 | name 1-63 chars, unique; chain at most 10 certificates; 256 KiB of PEM |
| CA bundles | at most 16 | name 1-63 chars, unique; 256 KiB of PEM |
| Expiry warning / dev regeneration | 14 days / 30 days | fixed |

## 6. Tasks

Task IDs are referenced by the dependency table in section 7. Workstream letters:
**G** groundwork, **MD** model, **S** store, **F** config file, **T** the TLS
crate, **CS** config service, **R** routing and compile, **D** data plane, **H**
health, **A** admin API, **U** admin UI, **Q** quality, **W** docs, **C** carry-over
from M2.

### Workstream G: Groundwork

- [x] **G1 (S) Decisions and ADRs.** Get the **(confirm)** items in section 3
  resolved. Write `docs/adr/`: 0009 TLS stack and the enablement model (F1, F4,
  F12, F19, F20), 0010 certificates: first-class, referenced, stored sealed in
  SQLite, history, API safety, limits (F2, F5, F6, F11, F24-F28), 0011 certificates
  in the snapshot, the cache, the write path and cluster readiness (F7-F10), 0012
  redirect, HSTS and the 421 rules (F13, F14), 0013 upstream TLS (F3, F15-F17), and a
  short note recording the ACME constraints of F29 for M4.
  *Done when:* the ADRs are merged and no **(confirm)** item is open.
  *Landed (PR1):* ADRs 0009-0014. The spike findings section of ADR 0009 is filled
  in by G2, which has not been run yet.
- [ ] **G2 (M) Spike: rustls + ring + hyper h2 + AES-GCM + snapshot-carried
  certificates on all three platforms.** A throwaway test that (a) builds and links
  `rustls` with `ring` on Linux, macOS and Windows CI, (b) serves a request over TLS
  through `tokio-rustls` into hyper's auto builder and gets HTTP/2 and HTTP/1.1 by
  ALPN, (c) selects a certificate in a `ResolvesServerCert` that reads an
  `arc-swap` snapshot holding `Arc<CertifiedKey>`s and swaps the snapshot while a
  handshake is in flight, (d) connects to a TLS server with `hyper-rustls` through
  the existing legacy client, (e) generates a certificate with `rcgen` (wildcard
  and exact SANs) and parses it back (SANs, validity, fingerprint), (f) seals and
  opens a value with `ring`'s AES-256-GCM with associated data, and (g) has two
  connections to one SQLite file see each other's committed revision version.
  Record findings (build time and size cost, any Windows problem) in ADR 0009.
  *Done when:* the findings are in ADR 0009 and F1 is confirmed or changed.
- [x] **G3 (S) Defaults table** (section 5.4). Turn F4-F14 and F28 into the documented defaults
  (section 5 and the config docs): the single source for validation and the UI.

### Workstream MD: Model

- [x] **MD1 (M) Subsystem HTTPS policy.** `HttpsPolicy { enabled, certificate_id,
  redirect, hsts }`, `CertificateId`, `RedirectMode` (off, 301, 308), `Hsts {
  enabled, max_age, include_subdomains }` with bounds, and the rule that redirect
  and HSTS settings need `enabled`.
  *Done when:* table-driven tests accept and reject each case with a
  field-specific message (`https.hsts.maxAgeSecs`, `https.redirect`).
- [x] **MD2 (M) Upstream TLS settings.** `UpstreamTls { enabled, verify,
  server_name, ca }` with a validated server name and CA name; the group policy
  gains it.
  *Done when:* tests cover every bound and the defaults match G3.
- [x] **MD3 (M) Certificate metadata, limits and `covers`.** `CertificateName`,
  `CertificateSource`, the metadata record (names, subject, issuer, validity,
  fingerprint), the limits of F28, and the pure `covers(san, pattern)` rule of F11,
  all without PEM or key types.
  *Done when:* tests cover name rules, each limit, and `covers` with a property
  test against a brute-force reference (exact, wildcard, one label only, wildcard
  host patterns, case).

### Workstream S: Store

- [x] **S1 (S) Schema.** Edit `schema.sql` in place with 5.2 (new subsystem and
  group columns, and the `certificate`, `ca_bundle` and `vault` tables); bump
  `SCHEMA_VERSION` to 3.
  *Done when:* a fresh database opens and a version-2 file is rejected with the
  existing clear error.
- [x] **S2 (M) Subsystem and upstream columns.** Repository create/update/load
  carry the HTTPS policy (including the certificate reference) and the TLS
  settings; load revalidates (corruption is `Internal`); the revision JSON carries
  both plus the certificate's name; rollback restores them, and a rollback that
  references a deleted certificate or bundle is refused with a message naming it.
  *Done when:* a test edits both, rolls back to each prior revision and compares
  `load_all()` to the stored configuration, and the deleted-reference case is
  covered.
- [x] **S3 (M) Certificate and CA bundle repositories.** Insert, replace, rename,
  delete (409 while used), list and load for both tables, plus the vault row; keys
  are stored and returned as opaque ciphertext and nonce; limits enforced; unique
  names are a `Conflict`.
  *Done when:* success and failure tests cover uniqueness, the used-certificate
  refusal, cascade and limits, and a test proves no function in the crate decrypts.
- [x] **S4 (S) Summary-only revisions.** A way to append a **summary-only** revision
  (the configuration unchanged) from a certificate write; `restore` leaves the
  certificate tables alone.
  *Done when:* a test uploads and deletes certificates, rolls back across them,
  and shows the certificates untouched and every change present in the history
  with no PEM in any revision row.
- [x] **S5 (S) The storage boundary.** The `CertificateSource` read trait
  (`load_all`, `version`) with the SQLite implementation, and a documented
  portability checklist for the certificate SQL (types, no SQLite-only features).
  *Done when:* the config service reads certificates only through the trait, and a
  test double implements it.

### Workstream F: Config file and command line

- [x] **F1 (S) TLS config and flags.** `TlsConfig` (`enabled`, `listen`,
  `public_port`, `min_version`, `http2`, `reject_unknown_sni`, `handshake_timeout`,
  `max_concurrent_handshakes`), `master_key_file`, `config_poll_interval`, and the
  CLI flags `--tls`, `--no-tls` (conflicting with each other) and `--https-listen`
  overriding the file; `deny_unknown_fields` still rejects anything else.
- [x] **F2 (S) Validation.** Every rule in 5.1, with errors that name the file and
  key (and never print key material); the flags override the file before
  validation; TLS on with no certificate is allowed.
  *Done when:* each rule has an accepted and a rejected test, the flag precedence is
  tested, and `config.toml` in the repo carries a commented TLS example.

### Workstream T: The TLS crate (new)

- [x] **T1 (M) PEM loading and validation.** Parse a chain and a key from memory
  (PKCS#8, PKCS#1, SEC1), reject an encrypted key, check the key matches the leaf,
  extract SAN names, subject, issuer, validity and the fingerprint, and classify
  expired and expiring certificates.
  *Done when:* tests (using rcgen-made certificates) cover good, expired,
  not-yet-valid, mismatched key, garbage, empty input, an encrypted key, a chain
  in the wrong order, and no usable DNS name; no error message contains PEM text.
- [x] **T2 (M) Key vault.** Load the master key (environment, else file, creating
  the file with random bytes and the right permissions when a key must be sealed),
  seal and open with AES-256-GCM binding `(id, name)`, the vault check value,
  zeroing of key buffers.
  *Done when:* tests cover a round trip, tampered ciphertext, a ciphertext moved
  to another row, a wrong master key (detected by the check), the file created
  once and reused, nothing created when nothing needs sealing, and the permission
  warning on Unix.
- [x] **T3 (M) Self-signed generator.** `rcgen` certificate for a list of host
  names and wildcard patterns (a subsystem's hosts), returning material for the
  service to store; and the in-memory ephemeral "unknown host" certificate.
  *Done when:* tests cover the SANs (exact and wildcard), the validity, that the
  result passes T1, and that the ephemeral certificate is never persisted.
- [x] **T4 (L) `CertHandle` and `CertCache`.** The handle (id, name, version, parsed
  rustls certified key, metadata); the cache keyed by id and version that, given
  the stored rows, **reuses unchanged entries, parses new or changed ones, drops
  removed ones and marks unreadable ones**; hit, miss and unreadable counters.
  *Done when:* tests cover a rebuild reusing every unchanged handle (pointer
  equality), a changed row producing a new handle while the old one stays usable,
  a removed row, an unreadable row not poisoning the others, a wrong master key
  marking all unreadable, and a rebuild with 256 certificates under a time budget.
- [x] **T5 (M) Server configuration.** Build the rustls `ServerConfig`: minimum
  version, ALPN from `http2`, a resolver supplied by the data plane, session
  tickets on and early data off, `ring` provider.
  *Done when:* a handshake test per case: TLS 1.2 and 1.3 accepted, a client
  limited to 1.2 refused when the minimum is 1.3, h2 and http/1.1 negotiated,
  h2 not offered when disabled.
- [x] **T6 (M) Client configurations.** `ClientConfig` per `(verify, ca)`: the OS
  trust store, an uploaded CA bundle, or no verification (with the warning);
  built once and cached, and rebuilt when a bundle is replaced; the server name
  chosen from the override or the endpoint host.
  *Done when:* tests cover trusted, untrusted, named CA, wrong name, an IP
  endpoint with and without an override, verification off, and a replaced bundle.
- [x] **T7 (S) Metrics and status.** `fastproxy_tls_handshakes_total{listener,
  result,version}`, `fastproxy_tls_cert_expiry_seconds{cert,source}`,
  `fastproxy_tls_cert_unreadable{cert}`, `fastproxy_cert_cache_total{result}`,
  `fastproxy_config_sync_total{result}`, a status snapshot for the admin plane;
  series of deleted certificates are deleted.
  *Done when:* a scrape after known events asserts each metric.

### Workstream CS: Config service

_Landed in PR4b: CS1 (without the `certState` broadcast, which arrives with the
admin events in PR5), CS2, CS4. D1 serves HTTP/1.1 only: ALPN offers `http/1.1`
until D2 (the process `http2` setting is wired then). The data plane already
uses the real scheme for `X-Forwarded-Proto` (part of D4). Q7 is partly covered
(database bytes, wrong key, key file created once and reused); its API, log and
loopback checks come with PR5._

- [x] **CS1 (L) Certificate and HTTPS writes.** Through the single writer: seal and
  store keys, apply HTTPS settings, **veto by compiling the would-be snapshot with
  the would-be certificates**, commit (summary-only revision for pure material
  changes), swap, broadcast `certState`. Covers upload, replace, rename, attach,
  detach, delete, generate and the HTTPS settings.
  *Done when:* tests cover each operation, a veto leaving everything as it was
  (including the cache), concurrent writes serialising with other configuration
  writes, and the swap being visible to a handshake already waiting.
- [x] **CS2 (M) The certificate guarantee.** The reconciler of F6: enabling HTTPS
  generates and attaches a dev certificate when none is attached, host add/remove
  regenerates a dev certificate that stops covering the hosts, detaching falls back
  to a dev certificate, startup fills gaps (only while TLS is on), the hourly sweep
  regenerates near expiry (clock passed in); an uploaded certificate is never
  replaced.
  *Done when:* tests cover each trigger, that nothing is generated with TLS off,
  that uploaded certificates are untouched, and that every write is atomic with its
  generated certificate.
- [ ] **CS3 (L) Cluster sync.** A poll task comparing the database version with the
  live snapshot's every `config_poll_interval`; on a newer version it reloads
  configuration and certificates, carries runtime state and the cache, and swaps;
  the instance's own commits never trigger a reload; failures are counted and
  retried; shutdown stops it.
  *Done when:* tests with **two config services on one database file** show a
  subsystem edit, a certificate upload, a renewal and a deletion on one appearing on
  the other within the interval, endpoint runtime state surviving the reload, no
  reload when nothing changed, and an interval of zero doing nothing.
- [x] **CS4 (M) Startup.** Unlock the vault when needed, build the cache from the
  rows (skipping and reporting unreadable ones), reconcile, compile, and expose the
  vault state (`ok`, `key mismatch`, `not created`).
  *Done when:* tests cover a fresh database, a database with certificates, a wrong
  master key (starts, reports, serves plain HTTP), and a corrupt row.

### Workstream R: Routing and compile

- [x] **R1 (L) Compile the new settings and certificates.** `CompiledSubsystem.https`
  (enabled, redirect, HSTS, the certificate handle) and `CompiledUpstream.tls`; the
  host table answers "which subsystem and certificate for this SNI"; the compile
  step takes the certificate handles, the CA bundle names and the process TLS state
  as inputs, and rejects an unknown `tls.ca`, a certificate reference that does not
  exist, and HTTPS enabled with no usable certificate while TLS is on, with clear
  errors (also on rollback).
  *Done when:* tests cover each rejection, the lookup for exact, wildcard and
  unknown names, a certificate handle swapped by a new compile, and that unrelated
  edits keep runtime state (as in M2).
- [x] **R2 (S) `build_upstream_uri`** takes the scheme from the group's TLS flag.
  *Done when:* the URI tests cover `https` with and without a port and IPv6.

### Workstream D: Data plane

- [x] **D1 (L) TLS accept path and resolver.** When TLS is on, start the HTTPS
  listener; wrap accepted connections in a handshake (timeout, concurrency bound);
  the snapshot-based resolver (SNI to subsystem to certificate, the ephemeral
  certificate for an unknown name, `reject_unknown_sni`); build the connection
  context (scheme, version, ALPN, SNI, certificate id); handshake metrics; hand the
  stream to the existing auto builder. Plain listeners are unchanged, and with TLS
  off nothing TLS-related is created.
  *Done when:* end-to-end tests over HTTP/1.1 cover the right certificate per
  subsystem, a shared wildcard certificate, an unknown name, no SNI,
  `reject_unknown_sni`, failed handshakes counted by reason, a stalled handshake cut
  off at the timeout, and a certificate swapped by a new snapshot reaching the next
  handshake only.
- [ ] **D2 (L) HTTP/2 to clients.** Enable h2 in the auto builder with the limits
  in F20, GOAWAY on graceful shutdown, and the connection tracker counting h2
  connections (and not each stream) for draining.
  *Done when:* tests show multiplexed requests on one connection, a stream limit
  enforced, a header-list limit answered, a rapid-reset attempt contained, and a
  drain letting open streams finish before the connection closes.
- [ ] **D3 (M) The 421 rules.** F14: a host of a subsystem with HTTPS disabled, and a
  host served by a different certificate than the connection's; no-SNI connections
  skip the second rule.
  *Done when:* tests cover the same name, two subsystems sharing one certificate
  (allowed), different certificates (421), HTTPS disabled (421), no SNI, an
  absolute-form URI, and a certificate replaced mid-connection.
- [ ] **D4 (S) Scheme-aware forwarding.** `X-Forwarded-Proto` and `Forwarded`
  from the connection; client-supplied values are still not trusted unless the
  peer is trusted; the upstream URI scheme follows the group.
  *Done when:* the forwarded-header tests cover http and https and trusted peers.
- [ ] **D5 (M) Redirect and HSTS.** The redirect for plain-HTTP requests (F13), with
  the `public_port` rule, only while TLS is on, and a single function the ACME
  challenge can bypass in M4; `Strict-Transport-Security` on HTTPS responses only.
  *Done when:* tests cover 308 and 301, the path and query preserved byte for
  byte, a non-default port, an unknown host (404, never a redirect), HTTPS
  disabled (no redirect), TLS off (no redirect even if configured), HSTS present
  on HTTPS and absent on HTTP, and `includeSubDomains`.
- [ ] **D6 (L) Upstream TLS.** `UpstreamClient` keeps pools per `(connect timeout,
  TLS settings)`; a `hyper-rustls` connector with the cached `ClientConfig`;
  passive health, retries and body replay keep working unchanged over TLS.
  *Done when:* end-to-end tests cover a TLS upstream with a trusted CA, an
  untrusted one (502 and a failure counted), a name mismatch, an uploaded CA
  bundle, an override name for an IP endpoint, a replaced bundle taking effect for
  new connections, and a retry across a TLS and a dead endpoint.
- [ ] **D7 (S) Access log and metrics.** `proto` (`http/1.1`, `h2`) and `tls`
  (version, or `-`) fields on data-plane lines; the exit-criteria and
  getting-started examples updated.
- [ ] **D8 (M) Handshake and connection hygiene.** Slowloris-style stalls,
  oversized client hellos, and a flood of failing handshakes cannot starve
  established traffic.
  *Done when:* a test holds many stalled handshakes and shows an established
  connection still served at normal latency.

### Workstream H: Health

- [ ] **H1 (S) TLS probes.** HTTP probes to a TLS group handshake with the group's
  settings (verify, name, CA); the TCP probe stays plain.
  *Done when:* tests mark a TLS endpoint healthy, and unhealthy for an untrusted
  certificate and for a plain-HTTP port.

### Workstream A: Admin API

_Landed in PR5: A1-A6, with the `certState` WebSocket event and activity entries,
and Q7 (its log-line check is by construction: no PEM or key is ever formatted
into a log, and every secret type has a redacted `Debug`; there is no log
capture test). A7 and A8 follow with upstream TLS and the redirect work._

- [x] **A1 (M) Reads.** `GET /api/certificates` (with `usedBy`), `GET /api/ca-bundles`
  and `GET /api/tls` (5.3): metadata only; never a key, a ciphertext or a PEM of
  a private key.
- [x] **A2 (L) Certificate writes.** `POST`, `PUT`, `DELETE` on certificates and
  `POST /api/certificates/self-signed`, through the config service; field errors
  (`certPem`, `keyPem`, `name`) that never echo PEM; the warning for a certificate
  expiring soon; 409 for a certificate in use.
- [x] **A3 (M) Subsystem certificate shortcuts.** `POST` and `DELETE
  /api/subsystems/{id}/certificate` (upload and attach in one step; detach), with
  the coverage rule (400 naming `https.certificateId` or `certPem`) and the
  uncovered-hosts warning.
- [x] **A4 (M) CA bundles.** `POST` and `DELETE /api/ca-bundles` (409 while an
  upstream names the bundle).
- [x] **A5 (M) Key safety.** The loopback-peer rule for requests that carry a
  private key (403 otherwise), PEM body size caps, and a test that no response,
  log line or event ever contains key material.
- [x] **A6 (M) Subsystem HTTPS settings** in the subsystem API (`https` object,
  including `enabled`, `certificateId`, `redirect`, `hsts`, the read-only
  `effective` and `certificate`), validated with nested field names
  (`https.hsts.maxAgeSecs`); redirect and HSTS without `enabled` are a 400;
  enabling generates a dev certificate when none is attached.
- [ ] **A7 (M) Upstream TLS settings** in the group API (`tls` object); an unknown
  CA name is a 400 naming `tls.ca`.
- [ ] **A8 (S) Resolver, status and events.** The resolver shows the scheme the
  proxy will use to the upstream, whether a request on plain HTTP would be
  redirected, and whether the host is served over HTTPS; `GET /api/status` adds the
  process TLS state and the soonest certificate expiry; the `certState` WebSocket
  event with an activity entry.

*Done for A1-A8:* every endpoint has HTTP-level tests in the style of
`api_tests.rs` (success, validation `fields`, 404, 409, 403 for the loopback
rule), and a test proves each write is visible to the data plane without a restart.

### Workstream U: Admin UI

- [ ] **U1 (S) API client, types and validation** for the new resources and fields
  (cheap PEM sanity checks: the text contains the expected `BEGIN` lines).
- [ ] **U2 (L) Subsystem HTTPS section.** An enable switch (explaining, and
  disabled-looking with the reason, while TLS is off for the process); a
  **certificate card** (name, source badge, issuer, expiry with days left, hosts it
  does not cover) with actions: **upload a certificate and key** for this subsystem
  (paste or choose a file read in the browser, the key field write-only and cleared
  after submit), **choose an existing certificate**, **generate a dev certificate**,
  detach; redirect (off, 308, 301) and HSTS controls with confirmations that say
  what will happen (HSTS in particular: browsers remember it for the max age, so a
  wrong setting is hard to undo); a placeholder where M4 adds the ACME switch.
- [ ] **U3 (M) Certificates page.** All certificates with source badge, expiry,
  state and which subsystems use them; upload, replace, rename and delete (the
  server's 409 shown); "generate self-signed"; the process TLS state, the vault
  state, and a banner while only dev certificates exist.
- [ ] **U4 (M) CA bundles section** on the same page: upload, list, delete (the
  server's 409 shown).
- [ ] **U5 (M) Upstream TLS fields.** Enable, verify (with a warning badge when
  off), server name, CA bundle chosen from the uploaded ones.
- [ ] **U6 (S) Dashboard.** A certificate card: the soonest expiry, a warning under
  14 days or after expiry, the dev-certificate warning, and the TLS-off notice;
  live updates from `certState`.
- [ ] **U7 (S) Revisions viewer and resolver** show HTTPS and TLS settings (and, as
  carry-over C2, the group policy that M2 left out); certificate changes appear in
  the history as summary entries.
- [ ] **U8 (S) Tests and build.** Vitest tests for the forms (validation shown,
  submit sends the right body, the key never rendered or kept), the confirmations,
  the certificate card and table, and live updates; `npm test` and
  `npm run build` pass.

### Workstream Q: Quality

- [x] **Q1 (M) Test PKI and TLS clients.** A test helper (rcgen, dev-dependency
  only) that makes a CA, leaf certificates with chosen names and validity, as PEM
  strings (uploaded through the API in end-to-end tests, so no files are involved);
  TLS clients for tests (HTTP/1.1 and HTTP/2, a chosen SNI, a chosen maximum
  version, trusting a chosen CA). No fixtures committed, no Unix-only commands, so
  tests run on Windows.
- [ ] **Q2 (L) Exit-criteria end-to-end tests** (`tests/tls.rs`), the real binary:
  TLS off by default and the flag and config switch turning the listener on;
  enabling HTTPS on a subsystem producing a dev certificate that the next handshake
  serves; uploading a real certificate for a subsystem and the next handshake
  serving it; two subsystems sharing a wildcard certificate; SNI selection and the
  unknown-name placeholder; HTTP/1.1 and HTTP/2 on one port; replace and delete
  taking effect with no restart, and an invalid upload changing nothing; a host of
  an HTTPS-disabled subsystem getting 421; the minimum version; redirect and HSTS
  live (and inert with TLS off); the 421 rule; a TLS upstream with an uploaded CA,
  trusted and not; TLS health checks; everything surviving a restart. Each must be
  shown to be able to fail.
- [ ] **Q3 (M) Live replacement under load.** Clients keep making new HTTPS
  connections and requests while certificates are uploaded, replaced and deleted
  repeatedly and subsystems toggle HTTPS (including invalid uploads): zero failed
  handshakes for names that stay enabled, every handshake gets a certificate that
  was valid at some point during it, and established connections keep working.
- [ ] **Q4 (M) HTTP/2 behaviour tests** beyond D2's: large bodies and flow control
  with a slow backend, many streams under retries, cancellation of one stream
  leaving the others alone, and the in-flight counts returning to zero.
- [ ] **Q5 (S) Benchmark.** Re-run `docs/benchmarks.md` for plain HTTP with TLS off
  (compare with M2) and with TLS on, and add HTTPS with keep-alive on HTTP/1.1 and
  HTTP/2, new-connection handshake rate with and without session resumption, the
  cost of a certificate write and of a cache rebuild with 256 certificates, and a
  TLS upstream; record the numbers.
- [ ] **Q6 (S) Gates.** fmt, clippy, tests and UI build green on Linux, macOS and
  Windows (the M2 lessons apply: wait for events, not durations; a refused
  connection is slow on Windows; one write per raw request).
- [x] **Q7 (M) Key protection tests.** The database file (read as bytes, including
  the WAL) contains no `PRIVATE KEY` text and none of the key bytes after
  uploads, replacements, deletions and dev-certificate generation; a wrong or
  missing master key is reported clearly and the proxy still starts serving plain
  HTTP and the certificates it can read; the master key file is created once,
  only when needed, and reused; no API response, log line, metric, event or
  revision contains key material; the loopback rule holds for a non-loopback peer.
- [ ] **Q8 (S) Security review.** A checklist run before release: keys never
  logged or served, errors never leak paths or PEM, the ephemeral certificate never
  served for a host that has a usable certificate, HSTS only over HTTPS, the
  redirect never builds a `Location` from anything but a matched host, handshake
  and h2 limits in force, `verify = false` visible everywhere, size and count
  limits in force. Findings recorded.
- [ ] **Q9 (M) Cluster behaviour tests.** Two instances (two config services, and
  once two real processes) on one database: a renewal, an upload, an HTTPS toggle
  and a deletion on one reach the other within the poll interval; the cache reuses
  unchanged certificates on the receiving side; endpoint runtime state survives a
  reload; a stale instance never writes over a newer database state (the veto reads
  inside the transaction); a different master key on the second instance is
  reported, not served.

### Workstream W: Documentation

- [ ] **W1 (M) README and getting-started.** A TLS walkthrough: turning TLS on
  (`--tls`), enabling HTTPS on a subsystem and seeing the dev certificate, uploading
  a generated certificate with `curl`, SNI with two subsystems, HTTP/2 with
  `curl --http2`, replacing a certificate live, redirect and HSTS, a TLS upstream
  with an uploaded CA; the config table; the metrics list; a **backup and master
  key** section (the database and `master.key` are backed up separately; losing the
  key means re-uploading); and a short **running several instances** section
  (shared database, the same master key, the poll interval, what stays local).
- [ ] **W2 (S) AGENTS.md.** The new crate and rules: certificate material and keys
  never appear in `Config`, revisions, logs, errors, metrics or API responses;
  certificate writes go through the config service like every other write; the
  `store` crate never decrypts; routing carries certificates in the snapshot, so a
  certificate or HTTPS change is one atomic swap; the self-signed certificate is
  generated only for subsystems that enable HTTPS.
- [ ] **W3 (S) ROADMAP.md.** Tick M3, record decisions F1-F29 in the decision log,
  update the risks table and the config example (certificates are in the database,
  not the file; ACME settings stay in the file), link this plan.
- [ ] **W4 (S) Outcome.** A closing section in this file like M2's section 11.

### Workstream C: Carry-over from M2

- [ ] **C1 (S) Owner click-through of the M2 UI** (the old C3) in a real browser,
  desktop and phone width, light and dark, **before** U2-U5 build on it.
- [ ] **C2 (S) Revision viewer shows group policies** (the old U6 gap); done with
  U7 above.
- [ ] **C3 (M) Find the unexplained flaky test.** One full-suite run failed once
  in M2 and did not reproduce. Run the suite in a loop and under load in CI
  (a scheduled job or a one-off branch) to identify it, and fix or document it.
- [ ] **C4 (S) Decide upstream HTTP/2** with data: measure what multiplexing to
  upstreams would buy on the M3 benchmark and record the decision (F3).
- [ ] **C5 (S) Merge cadence.** M2's stack was merged only at the end; for M3 merge
  each PR to `main` as it passes CI so the stack stays short (see section 7).

## 7. Dependencies and suggested order

| Task | Depends on |
|---|---|
| G1 | none (needs the owner's answers) |
| G2 | none (a spike; can start now) |
| G3 | G1 |
| MD1, MD2, MD3 | G3 |
| S1 | MD1-MD3 |
| S2 | S1 |
| S3 | S1, MD3 |
| S4 | S3 |
| S5 | S3 |
| F1-F2 | G3 |
| T1 | G2, MD3 |
| T2 | G2 |
| T3 | T1, MD3 |
| T4 | T1, T2 |
| T5 | T1 |
| T6 | T1, S3 |
| T7 | T4 |
| CS1 | T4, S2-S5, R1 |
| CS2 | CS1, T3 |
| CS3 | CS1, S5 |
| CS4 | CS1, T2, T4 |
| R1 | MD1, MD2, T4 (the handle type) |
| R2 | MD2 |
| D1 | T5, F2, R1, CS4 |
| D2 | D1 |
| D3 | D1 |
| D4 | D1, R2 |
| D5 | D1, R1 |
| D6 | T6, R1, R2 |
| D7 | D1, D2 |
| D8 | D1 |
| H1 | D6 |
| A1 | CS1, T7 |
| A2 | CS1, A5 |
| A3 | CS1, A5 |
| A4 | S3, T6 |
| A5 | CS1 |
| A6 | CS1, CS2 |
| A7 | S2, R1 |
| A8 | R2, D5, T7 |
| U1 | A1 (shapes can be agreed up front) |
| U2 | A1, A3, A6 |
| U3 | A1, A2 |
| U4 | A4 |
| U5 | A7, A4 |
| U6 | A1, A8 |
| U7 | A6, A7 |
| U8 | U2-U7 |
| Q1 | none (test support) |
| Q2 | D1-D6, H1, A1-A6, CS2, Q1 |
| Q3 | CS1, D1, A2, Q1 |
| Q4 | D2, Q1 |
| Q5 | D2, D6, CS3 |
| Q6 | every code task (runs in CI on each PR) |
| Q7 | S3, S4, T2, CS1, CS2, A5 |
| Q8 | every code task |
| Q9 | CS3, T4, Q1 |
| W1-W4 | feature tasks complete |
| C1 | none (the owner's check; do it before U2) |
| C2 | U7 |
| C3 | none |
| C4 | Q5 |
| C5 | n/a (process) |

Suggested slicing into pull requests, each leaving `main` green and releasable.
**Merge each to `main` as it passes CI** (C5), so the stack stays at two or three:

1. **PR1** G1, G3 (decisions and ADRs); G2 as its own throwaway branch.
2. **PR2** MD1-MD3, S1-S5 (the HTTPS policy and `covers`, upstream TLS settings,
   the certificate, CA bundle and vault tables, summary-only revisions, the storage
   boundary; schema 3): no behaviour yet.
3. **PR3** T1-T4, T6, T7 (the pure TLS crate: PEM validation, key vault,
   self-signed generator, `CertCache`, client configurations, metrics) with Q1
   (test PKI), in parallel with PR2.
4. **PR4** T5, R1, F1-F2, CS1, CS2, CS4 and D1 (server configuration, certificates in
   the snapshot, the config section and flags, the write path, the certificate
   guarantee, startup, and the TLS accept path with the resolver, HTTP/1.1 over
   TLS) with `tests/tls.rs` basics and the key protection tests (Q7): TLS works end
   to end, driven by the config for now.
5. **PR5** A1-A6 (certificate, CA bundle and subsystem HTTPS API, loopback rule):
   enable HTTPS on a subsystem, upload a certificate and see it served.
6. **PR6** CS3, Q9 (the cluster sync and its tests).
7. **PR7** D2, D3, D4, D7, D8 (HTTP/2, the 421 rules, scheme-aware forwarding,
   access log, handshake hygiene).
8. **PR8** D5, A8 (redirect and HSTS, resolver and status).
9. **PR9** R2, D6, H1, A7 (upstream TLS, TLS probes, group API).
10. **PR10** U1-U8 (UI), after the owner's C1 click-through.
11. **PR11** Q2-Q6, Q8, W1-W4, C2-C4, release `v0.4.0`.

**Split of PR4 (decided while implementing):** PR4a is T5, R1, R2, F1-F2 (the
server configuration, the compile step carrying HTTPS settings and certificates,
the config section and flags); PR4b is CS1, CS2, CS4, D1, the `tests/tls.rs`
basics and Q7 (the write path, startup and the accept path). Both stay stacked.

PRs 2 and 3 can proceed in parallel after PR1; PR4 needs PR2 and PR3; PRs 5 to 9
need PR4 and can be taken in a different order (PR6 needs PR5's write paths only
for its tests).

## 8. Performance and cost budget

- **Plain HTTP with TLS off must not slow down:** within about 5% of the M2
  numbers in [benchmarks.md](benchmarks.md) (one endpoint, 64 connections: about
  98k rps, about 27 us added at one connection). The accept path gains a branch on
  the TLS switch, and the pipeline a redirect check that must be a field read.
  With TLS **on**, plain-HTTP requests also stay within about 5%.
- **HTTPS keep-alive** should cost mostly the symmetric crypto: record the
  throughput and the added latency (expect a clear drop on loopback, where the
  proxy and the load generator share the CPU; report it, do not hide it).
- **Handshakes:** record full handshakes per second and resumed handshakes per
  second per core; session tickets must make resumption visibly cheaper. The
  resolver adds one snapshot load and one host lookup per handshake.
- **Certificate writes and sync:** a write rebuilds the snapshot with the cache
  reusing every unchanged certificate, so only changed certificates are parsed; a
  rebuild with 256 certificates must stay in the low milliseconds and a full
  parse of all 256 well under a second. The sync poll is one cheap query per
  interval. A change must not stall requests: the swap is one pointer swap and
  parsing happens off the request path.
- **Startup:** decrypting and parsing up to 256 certificates must stay well under
  a second.
- **Memory:** decrypted keys exist once, in the cache; no per-connection
  allocation beyond what rustls needs; the connection tracker still counts
  connections, not streams.

## 9. Risks specific to M3

| Risk | Mitigation |
|---|---|
| **Private keys leaking through the database file, a backup, or a copy of the state directory** | Keys are sealed with AES-256-GCM under a master key kept outside the database (F24, F25); the database alone is not enough; Q7 reads the raw file and WAL for key bytes; the docs say to back the two up separately |
| **Keys leaking through revision history** (the full configuration is copied per change) | Certificate material is not part of `Config`; material changes are summary-only revisions (F26); tested (S4, Q7) |
| **The admin plane has no authentication until M5, and it now accepts private keys** | Key-bearing requests are accepted only from a loopback peer (F27); the admin listener already defaults to loopback and warns otherwise; keys are write-only; the limitation is stated in the docs and the UI |
| **Instances drifting apart or serving different certificates** | The revision version is the generation and every instance polls it (F9); the window is the poll interval and is documented; Q9 tests it with two instances; a renewal is safe in the window because both certificates are valid |
| **Two instances with different master keys** | The vault check detects it at startup and reports it; the instance serves plain HTTP and the certificates it can read; documented loudly (F25) |
| **Two instances writing at once** | SQLite serialises transactions; the veto and the snapshot are built from state read **inside** the transaction, so a stale instance cannot validate against an old view; the writer lock only orders one process's writes (documented); sharing one SQLite file over a network filesystem is unsupported, and PostgreSQL is the later answer |
| **A certificate or HTTPS write racing other configuration writes** | The same single writer and lock as every configuration write (F10); concurrent-write tests (CS1) |
| **A subsystem enabling HTTPS and a request arriving before it has a certificate** | The certificate is generated or attached in the same write that enables HTTPS (F6), startup fills gaps, the compile veto rejects HTTPS without a usable certificate, and the resolver falls back to the ephemeral certificate and 421 rather than failing open |
| **A dev certificate that no longer covers a subsystem's hosts after a host change** | Regenerated in the same write as the host change (CS2); the UI shows uncovered hosts |
| **The wrong certificate attached to a subsystem** | The coverage rule (F11): reject when it covers none of the subsystem's hosts, warn on partial coverage |
| Losing the master key makes stored keys unreadable | Detected at startup and reported; other certificates and plain HTTP keep working; dev certificates regenerate; the docs and UI say how to recover (upload again) |
| A corrupt or unreadable stored certificate blocking startup | Skipped and reported, never fatal (F8, F11); a status entry and a metric show it |
| Memory exposure of decrypted keys (core dumps, swap) | Keys are decrypted once into the cache's rustls structures and buffers are zeroed where the crates allow; documented as a limit, not hidden |
| An enormous or malicious PEM upload | Size caps, count limits, chain-length limit (F28), parsing off the request path |
| `ring` or `rustls` build trouble on Windows or macOS CI, or a large binary-size and build-time cost | Spike G2 first on all three platforms; the provider is a one-line swap |
| Routing now depending on the TLS crate | The dependency is data only (the handle type), no I/O; the alternative (a separate certificate swap) risks a handshake seeing a new setting with an old certificate, which the single snapshot swap avoids (F7) |
| HTTP/2 denial-of-service (rapid reset, header floods, stream floods) | Explicit limits (F20), rapid-reset mitigation on, tests in D2/Q4, GOAWAY drain |
| The 421 rule breaking legitimate HTTP/2 connection coalescing | Compare **certificates**, not names (F14): hosts under one certificate pass, and 421 is the standard signal to retry on a new connection |
| HSTS set wrongly locks users out for the max age | The UI confirmation says so plainly (U2); HSTS is opt-in, off by default, only accepted with HTTPS enabled, and only sent over HTTPS |
| A redirect to a listener that does not exist (TLS off) | Redirects are effective only while TLS is on (F13); tested (D5) |
| An open redirect through the `Location` header | The target is built only from a host that matched a configured subsystem pattern, plus the original path and query; tested (D5, Q8) |
| `verify = false` used and forgotten | Warning on every load, a UI badge, and a metric label (F16) |
| A rollback needing a certificate or CA bundle that was deleted | Refused with a message naming it (F26) |
| The self-signed certificate mistaken for a real one | `source` shown in status, metrics and the UI, a dashboard warning, generated only for subsystems that enable HTTPS (F6) |
| The OS trust store differs per platform (tests and users) | Tests always upload an explicit CA; the OS store is only the default for users |
| Time-dependent tests (expiry, regeneration, polling) are slow or flaky | Components take the clock as a parameter; the sync interval is configurable down to milliseconds in tests; end-to-end tests wait for events with generous limits |
| Windows CI differences (a refused connection takes about a second) | Waits are event-driven; no certificate files are involved (PEM goes through the API); no Unix-only commands |
| Scope creep into ACME (M4), PostgreSQL, mTLS, OCSP or KMS | New ideas go to the roadmap, not this plan; section 2 lists what is out, and F29 pins the ACME constraints so M4 does not reopen M3's design |
| The unexplained M2 test flake returns during TLS work | C3 hunts it with loops and load before it can mask a TLS regression |

## 10. Definition of done for M3

- [ ] Every task above is checked, or explicitly moved with a note.
- [ ] Exit criteria (section 1) demonstrated by the end-to-end tests in Q2, Q3, Q7
  and Q9 and a manual run: start with `--tls`, enable HTTPS on a subsystem (a dev
  certificate appears), upload a real or generated certificate in the UI and see
  the next handshake serve it, serve HTTP/2, replace the certificate live, enable
  the redirect and HSTS, put a TLS backend behind an upstream with an uploaded CA,
  and watch a second instance on the same database pick a change up.
- [ ] CI green on all three platforms; no new clippy warnings.
- [ ] Overall test coverage above 60%, number recorded (AGENTS.md rule); aim to
  stay near M2's 97.7%.
- [ ] Nothing committed without the owner's go-ahead after manual review.
- [ ] README, AGENTS.md, ROADMAP.md and getting-started updated, including the
  backup and master key guidance and the running-several-instances notes.
- [ ] Benchmark re-run and recorded; plain-HTTP overhead within budget.
- [ ] Key protection verified (Q7) and the security review (Q8) done, findings
  recorded.
- [ ] Release `v0.4.0` prepared through the existing release workflow.
