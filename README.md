# fast-proxy

[![CI](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml/badge.svg)](https://github.com/prasenjit-net/fast-proxy/actions/workflows/ci.yml)

A fast, **realtime-configurable ingress server and reverse proxy** written in
Rust, shipped as a single binary with an embedded admin UI.

You run one process, open the admin UI, and describe what to serve: each host
name is a **subsystem** with its own upstreams and routes. Changes apply to live
traffic immediately, without a restart and without dropping a request, and every
change is recorded so it can be rolled back.

> **Status: milestones 1 and 2 are feature-complete.** It is a working HTTP
> reverse proxy with live configuration, load balancing across several endpoints
> per upstream, active and passive health checking, and safe retries. TLS, ACME
> and maintenance mode are the next milestones; see the [roadmap](docs/ROADMAP.md). The admin plane needs a
> bearer token (see [Security notes](#security-notes)).

## What it does today

- **Host-based virtual hosting.** A request is matched to a subsystem by its
  `Host` header: exact names first, then the longest `*.wildcard` suffix.
- **Prefix routing.** Inside a subsystem the longest matching path prefix wins,
  on path-segment boundaries (`/api` matches `/api/x`, not `/apix`), with
  optional prefix stripping, an upstream path prefix, and per-route timeouts.
- **Live configuration.** Subsystems, hosts, upstreams and routes live in a local
  SQLite file and are edited through the admin UI or REST API. A change is
  validated, saved, and swapped in atomically; an invalid change is rejected and
  the old configuration keeps serving. It survives restarts.
- **History and rollback.** Every change is a numbered revision; any earlier
  state can be viewed and re-applied.
- **Upstream groups.** An upstream holds up to 64 endpoints with weights, spread
  by round robin, weighted round robin, least connections, power-of-two-choices
  on latency, or consistent hash (client IP, header or cookie).
- **Health and failover.** Active HTTP/TCP checks with rise/fall thresholds, and
  passive ejection of endpoints that keep failing (with backoff, and never more
  than half of a group). Failed attempts are retried on another endpoint under
  per-route rules and a per-group retry budget. If nothing is healthy a group
  fails open or answers 503, as configured.
- **Operator control.** Drain or disable an endpoint: it gets no new requests,
  in-flight ones finish, and the state survives a restart.
- **A streaming proxy.** Request and response bodies stream without buffering,
  upstream connections are pooled, WebSocket and other protocol upgrades pass
  through, and `X-Forwarded-*`, hop-by-hop headers and request ids are handled
  (client-supplied forwarding headers are not trusted unless the peer is listed
  in `trusted_proxies`).
- **TLS termination.** With `--tls` (or `[data_plane.tls] enabled = true`) an HTTPS
  listener starts next to the plain ones. Each subsystem opts in to HTTPS and gets
  **its own certificate by SNI**, over HTTP/1.1 or HTTP/2 negotiated on the one
  port. Certificates are uploaded, replaced and deleted **live** in the admin UI or
  API (no restart, no dropped connection), stored in the database with their
  private keys encrypted, and a subsystem that enables HTTPS without one gets a
  self-signed development certificate at once. A subsystem can redirect plain HTTP
  to HTTPS and send HSTS. A host whose certificate does not match the connection
  gets 421.
- **TLS to upstreams.** An upstream group can talk TLS to its endpoints, verified
  against an uploaded CA bundle (or the OS trust store), with an optional server
  name for IP endpoints; health checks use the same TLS settings.
- **Several instances, one database.** Instances sharing a database converge on
  the same configuration and certificates within a few seconds (see
  [Running several instances](#running-several-instances)).
- **A protected admin plane.** Every admin call needs a bearer token (roles `admin`
  and `viewer`, hashed at rest, shown once, revocable at once); the first start prints
  a bootstrap token, and the CLI can make one from the machine if every token is lost.
  The plane can serve HTTPS with a stored certificate, refuses a foreign `Host`
  header, throttles repeated failures and sends a strict Content-Security-Policy.
- **An audit log.** Every change records who made it, from where and what it did, in
  the same transaction as the change; refusals and failed attempts are recorded too.
  It is browsable, filterable and exportable, and never holds a secret.
- **Maintenance mode.** Close the whole proxy, a subsystem or one route with a 503, a
  message (or your own page), `Retry-After`, a TTL that ends it by itself, and a bypass
  by network or secret header so you can verify a deploy before reopening. It survives
  a restart, follows across instances, never touches an upstream and leaves health
  checks running.
- **A CLI and a complete admin UI.** `fast-proxy maintenance`, `reload`, `status`,
  `audit`, `config validate` and the offline `token` commands; the UI adds sign-in,
  tokens, the audit log, maintenance controls with a live countdown and banner, and a
  routes and upstreams overview, read-only for a `viewer`.
- **Graceful shutdown.** Ctrl-C stops accepting, closes idle connections, and
  lets in-flight requests and open WebSockets finish for up to
  `data_plane.shutdown_grace`.
- **Observability.** Prometheus metrics at `/metrics`, one access-log line per
  request, live traffic stats and a dashboard with upstream health, a live status
  view per upstream, and a resolver that shows exactly where a given host and
  path would be sent and which endpoints would take it.

## Quick start

Prerequisites: Rust (stable) and the Node version in `ui/.nvmrc`.

```sh
# 1. Build the frontend (it is embedded into the binary by the Rust build)
cd ui && npm install && npm run build && cd ..

# 2. Build and run the single binary
cargo run --release
# admin UI + API  -> http://127.0.0.1:9000
# the proxy       -> http://0.0.0.0:8000
```

Or just `make build` / `make run`. The first start prints an **admin token** once
(`admin token (shown once, store it now): fp_1.…`). Open the admin UI and paste it
into the sign-in page; it is kept for that browser tab only. A `viewer` token shows
the UI read-only (every control that would change something is hidden), and the
**Tokens** page (admins) creates and revokes tokens, showing a new one's secret once.
The **Audit log** page shows who changed what and who was refused, with filters and an
NDJSON export. Then create a subsystem,
add its host name, an upstream (your backend's `host:port` as its first
endpoint) and a route with prefix `/`, and send a request with that `Host` header to port 8000.

**[docs/getting-started.md](docs/getting-started.md)** walks through the same
thing with `curl`, step by step: two subsystems, live edits, the resolver,
rollback, metrics, WebSockets, shutdown, restart, and then several endpoints in
one upstream: kill one, watch it leave and return, drain another.

### Development (hot reload)

Run the two dev servers side by side:

```sh
cargo run                # terminal 1: backend (admin plane on :9000)
cd ui && npm run dev     # terminal 2: Vite on :5173, proxies /api and /ws
```

Open http://localhost:5173. In debug builds `rust-embed` reads `ui/dist` from
disk, so a production `npm run build` is picked up by a plain restart too.

## How it works

Two planes on two listeners:

```
                 config.toml                         SQLite file
                     |                                   ^
                     v                                   | (one writer thread)
  +----------------- main ------------------------------+ |
  |                                                      | |
  |  data plane (hyper)             admin plane (axum)   | |
  |  0.0.0.0:8000                   127.0.0.1:9000       | |
  |     |                           REST + WS + UI       | |
  |     v                           /metrics             | |
  |  load Snapshot (arc-swap) <--- swap -- ConfigService | |
  |     |                                (validate, save, compile, swap)
  |  host -> subsystem -> prefix route -> upstream group |
  |     |   pick an endpoint (algorithm, health, state)  |
  |  pooled client, retries --> your backends            |
  |     ^  active probes + passive ejection feed health  |
  +------------------------------------------------------+
```

- **The data plane** serves public traffic. Each request loads the current
  immutable routing *snapshot* once and uses it to the end, so a config change
  never affects a request already in flight. It never reads the database.
- **The admin plane** is the UI, REST API, WebSocket push and `/metrics`. It
  binds to loopback by default. All writes go through one `ConfigService`:
  run the change in a transaction, compile the would-be configuration (this is
  the validation), commit with a revision, swap the new snapshot, notify the UI.
- **Configuration is split on purpose.** `config.toml` holds how the process is
  hosted (listeners, state directory, admin address, logging) and needs a
  restart to change. Subsystems, hosts, upstreams and routes live in the
  database and change live. The file never contains routes; the database never
  contains listeners.

The design and the reasoning behind it are in [docs/ROADMAP.md](docs/ROADMAP.md)
and the decision records in [docs/adr/](docs/adr/).

## Capabilities and roadmap

| Capability | Status | Milestone |
|---|---|---|
| Subsystems by host name (exact and wildcard) | Done | M1 |
| URI prefix routing with strip/rewrite and timeouts | Done | M1 |
| Realtime configuration with history and rollback | Done | M1 |
| WebSocket and protocol-upgrade passthrough; graceful draining | Done | M1 |
| Prometheus metrics, access log, live traffic stats, admin UI | Done | M1 |
| Upstream groups with several endpoints, five load-balancing algorithms | Done | M2 |
| Active and passive health checking, safe retries, drain and disable | Done | M2 |
| TLS termination (rustls, SNI, live certificates, HTTP/2), TLS to upstreams, redirect and HSTS | Done | M3 |
| ACME automatic certificates | Planned | M4 |
| Admin authentication, admin TLS, maintenance mode, audit log, CLI, UI pages | Done | M5 |
| Request sampling of unusual requests | Planned | M6 |
| Rate and connection limits, hardening, 1.0 | Planned | M7 |

## CLI

With no subcommand `fast-proxy` runs the server. The flags below override the
config file and go **before** a subcommand; `--config` and `--database` also work
after one.

```
fast-proxy [OPTIONS] [COMMAND]

  -c, --config <CONFIG>             Path to the TOML configuration file [default: config.toml]
      --admin-listen <ADDR>         Override [admin].listen
      --admin-auth <required|off>   Override [admin].auth (off only on loopback)
      --admin-tls                   Serve the admin plane over HTTPS
      --database <PATH>             Override [data_plane].database
      --state-dir <PATH>            Override [data_plane].state_dir
      --log-level <LOG_LEVEL>       Override [logging].level (trace, debug, info, warn, error)
      --access-log <PATH>           Override [logging].access_log file path
      --no-access-log               Disable the access log file entirely
      --tls / --no-tls              Turn TLS on or off, overriding [data_plane.tls].enabled
      --https-listen <ADDR>         Override [data_plane.tls].listen (the HTTPS address)
```

### Subcommands

Operations on a **running proxy** talk to its admin API. They find it from the
`[admin]` section of the config file (or `--url`), and take a token from
`--token`, `--token-file` (refused on Unix unless only you can read it), or
`FAST_PROXY_TOKEN`, in that order of precedence (prefer the file or the variable: a
`--token` flag shows in the process list). For an admin plane served over
TLS, `--ca-file <pem>` trusts the CA that issued its certificate and
`--insecure` accepts a self-signed one. Add `--json` for the proxy's own answer.

```sh
export FAST_PROXY_TOKEN=fp_1...                      # the token printed on first start

fast-proxy maintenance on --ttl 30m --message "Upgrading the database" \
    --bypass-cidr 10.0.0.0/8                         # the whole proxy
fast-proxy maintenance on --subsystem shop --route /api --ttl 5m
fast-proxy maintenance status                        # what is on, with countdowns
fast-proxy maintenance off --subsystem shop
fast-proxy reload                                    # re-read the database now
fast-proxy status                                    # version, listeners, admin protection
fast-proxy audit --limit 20 --actor ci               # the audit tail, oldest first
```

| Command | Works on | Does |
|---|---|---|
| `maintenance on\|off\|status` | a running proxy | `--subsystem <name\|id>`, `--route <prefix>` choose the scope (default: the whole proxy); `on` takes `--ttl`, `--message`, `--retry-after`, `--bypass-cidr` (repeatable), `--bypass-header` and `--bypass-secret` |
| `reload`, `status`, `audit` | a running proxy | as above |
| `config validate [--check-database [PATH]]` | files only | parses and checks the config file with the command-line overrides applied, **creating nothing**; with `--check-database` it also opens the database **read-only** and compiles the stored configuration (certificates stay sealed, so HTTPS settings are not checked against them, and the output says so) |
| `token create\|list\|revoke` | the database file | the way back in when every token is lost: it works directly on the file (also while the proxy runs, which picks the change up at its next sync), prints a new token **once**, never lists a secret, writes an audit entry as `cli`, and keeps the last admin token from being revoked |

Exit codes: **0** success, **1** the proxy refused the request or the check
failed (the message carries the proxy's own reason and names each bad field),
**2** usage or connection errors (an unreachable proxy, a bad flag, a missing file).

## Configuration (`config.toml`)

| Key | Default | Meaning |
|---|---|---|
| `data_plane.state_dir` | `./state` | Directory for the database and other state; created at startup |
| `data_plane.database` | `<state_dir>/fast-proxy.db` | Database file |
| `data_plane.trusted_proxies` | `[]` | CIDRs whose `X-Forwarded-*` headers are trusted and appended to |
| `data_plane.shutdown_grace` | `30s` | Time in-flight requests get to finish on shutdown |
| `data_plane.listener` | one `http` listener on `0.0.0.0:8000` | Array of tables with `name` and `listen`; names and addresses must be unique |
| `data_plane.master_key_file` | `<state_dir>/master.key` | The key that encrypts stored private keys; created the first time one must be stored. `FAST_PROXY_MASTER_KEY` (base64 of 32 bytes) overrides it |
| `data_plane.config_poll_interval` | `5s` | How often to look for changes made by another instance; `0s` turns the sync off |
| `data_plane.tls.enabled` | `false` | The process-wide TLS switch (`--tls` / `--no-tls`) |
| `data_plane.tls.listen` | `0.0.0.0:8443` | The HTTPS listener (`--https-listen`); use 443 in production; must not clash with another listener |
| `data_plane.tls.public_port` | the listen port | The port redirects to HTTPS name (NAT, containers) |
| `data_plane.tls.min_version` | `"1.2"` | Lowest TLS version: `"1.2"` or `"1.3"` |
| `data_plane.tls.http2` | `true` | Offer HTTP/2 through ALPN |
| `data_plane.tls.reject_unknown_sni` | `false` | Fail the handshake for an unknown name instead of presenting a placeholder certificate |
| `data_plane.tls.handshake_timeout`, `max_concurrent_handshakes` | `10s`, `256` | Bounds on handshakes so a flood cannot starve established traffic |
| `data_plane.tls.http2_max_concurrent_streams`, `http2_max_header_list_size`, `http2_keep_alive_interval`, `http2_keep_alive_timeout` | `128`, `65536`, `30s`, `20s` | HTTP/2 limits |
| `admin.listen` | `127.0.0.1:9000` | Admin plane address; must differ from every listener. Leaving loopback needs `admin.tls.enabled = true` (or, to accept clear-text tokens, `admin.allow_insecure_remote = true`) |
| `admin.auth` | `required` | `required`: every API call and the WebSocket need a token; `off` is accepted only on loopback and is warned about |
| `admin.allowed_hosts` | `[]` | Extra names accepted in the `Host` header (the listen address and `localhost`, `127.0.0.1`, `[::1]` always are) |
| `admin.allow_insecure_remote` | `false` | Accept a non-loopback admin listener without TLS; tokens then cross the network in clear text |
| `admin.tls.enabled` | `false` | Serve the admin plane over HTTPS (the plain port is then not open). `--admin-tls` turns it on |
| `admin.tls.names` | `["localhost"]` | Names and IP addresses of the self-signed certificate generated the first time it is needed |
| `admin.tls.certificate` | `admin` | The stored certificate to serve, by name. Upload a certificate and key of that name on the Certificates page to replace the generated one, live |
| `logging.level` | `info` | Tracing filter (full directives allowed, e.g. `info,access=off`) |
| `logging.access_log` | *(unset)* | Access-log file path; omit to disable the file |
| `ui.app_name` | `Fast Proxy` | Shown in the sidebar + browser title |
| `ui.tagline` | ... | Shown under the app name |
| `ui.default_theme` | `auto` | `light` \| `dark` \| `auto`; used until the visitor picks |
| `ui.repo_url` | *(unset)* | Sidebar "Repository" link |

If the config file is missing, built-in defaults are used (with a warning).
Unknown keys are rejected, so a typo (or a certificate or key path, which never
belong in this file) fails at startup instead of being ignored. Every listener is bound before any
traffic is served, so a bad address or a port in use aborts startup with a clear
message.

## API

All routes except `/api/health` need `Authorization: Bearer <token>`; a missing or wrong token is 401, a `viewer` token on anything but a read is 403.

| Method | Path | Description |
|---|---|---|
| GET | `/api/health` | Liveness + version (open) |
| GET | `/api/whoami` | The caller's token name and role, its expiry, and whether the channel is protected |
| POST | `/api/ws-ticket` | A single-use ticket (30 s) to open `/ws?ticket=…`: browsers cannot set headers on a WebSocket |
| GET, POST | `/api/tokens` | List / create (`name`, `role`, optional `expiresInDays`; the secret appears only in the creation response) |
| DELETE | `/api/tokens/{id}` | Revoke at once (409 for the last usable admin token) |
| POST | `/api/reload` | Re-read the database now and recompile; answers with the live version |
| GET | `/api/audit?limit=&before=&actor=&action=&outcome=&since=` | The audit log, newest first, paged (`nextBefore`); readable by a `viewer` |
| GET | `/api/audit/export?since=` | Every entry since a time as NDJSON, oldest first, streamed (admin only) |
| GET | `/api/maintenance` | Every maintenance scope that is on, with its target names, message, `secondsLeft`, who set it and its bypass rules (never a secret) |
| PUT, DELETE | `/api/maintenance/global` | Turn maintenance on (replacing earlier settings) or off for the whole proxy |
| PUT, DELETE | `/api/subsystems/{id}/maintenance` | The same for one subsystem |
| PUT, DELETE | `/api/subsystems/{id}/routes/{route_id}/maintenance` | The same for one route |
| GET | `/api/config` | UI bootstrap config (the `[ui]` table, camelCased) |
| GET | `/api/status` | Listeners, database path, subsystem counts, upstream health summary, uptime, live config version |
| GET | `/api/traffic` | Latest live traffic sample: requests/s, 5xx error rate, connections, per subsystem |
| GET | `/metrics` | Prometheus text exposition (admin listener only) |
| GET, POST | `/api/subsystems` | List (host patterns, route counts) / create |
| GET, PUT, DELETE | `/api/subsystems/{id}` | Detail (hosts, upstreams, routes in match order) / replace / delete |
| POST | `/api/subsystems/{id}/hosts` | Add a host pattern (`pattern`) |
| DELETE | `/api/subsystems/{id}/hosts/{host_id}` | Remove a host pattern |
| GET, POST | `/api/subsystems/{id}/upstreams` | List / create a group: `name`, `endpoints` (`addr`, `weight`, `state`), and optional `lb`, `failurePolicy`, `health`, `passive`, `retryBudget` |
| PUT, DELETE | `/api/subsystems/{id}/upstreams/{upstream_id}` | Replace the name and policy (endpoints are separate) / delete (409 while a route uses it) |
| POST | `/api/subsystems/{id}/upstreams/{upstream_id}/endpoints` | Add an endpoint (`addr`, `weight`, `state`) |
| PUT, DELETE | `.../endpoints/{endpoint_id}` | Replace / remove (409 for the last endpoint of a group) |
| PUT | `.../endpoints/{endpoint_id}/state` | `{ "state": "enabled" \| "draining" \| "disabled" }`: takes effect for the next request |
| GET | `.../upstreams/{upstream_id}/status` | Live state per endpoint: effective state and why, in-flight, latency, last probe |
| GET, POST | `/api/subsystems/{id}/routes` | List in match order / create (optional `retry`: `maxAttempts`, `idempotentOn5xx`, `onTimeout`) |
| PUT, DELETE | `/api/subsystems/{id}/routes/{route_id}` | Replace / delete |
| GET | `/api/revisions?limit=&before=` | Revision history, newest first (paged) |
| GET | `/api/revisions/{version}` | The full configuration as of that revision |
| POST | `/api/revisions/{version}/rollback` | Re-apply a revision as a new one |
| POST | `/api/resolve` | Dry-run `{host, path}` against the live config: subsystem, route, exact upstream URL, or why nothing matched |
| GET | `/api/tls` | The process TLS switch, listener, minimum version and the key vault state (`ok`, `key mismatch`, `not created`) |
| GET, POST | `/api/certificates` | List (metadata only: names, validity, state, `usedBy`; **never a key**) / upload `{name, certPem, keyPem}` |
| PUT, DELETE | `/api/certificates/{id}` | Rename and/or replace the chain and key (a renewal may leave the key out) / delete (409 while a subsystem uses it) |
| POST | `/api/certificates/self-signed` | Generate a self-signed certificate `{name, names}` |
| POST, DELETE | `/api/subsystems/{id}/certificate` | Upload a certificate and attach it in one step / detach (a development certificate takes over while HTTPS is on) |
| GET, POST | `/api/ca-bundles` | List / upload `{name, pem}` CA bundles that upstream groups name to verify TLS backends |
| DELETE | `/api/ca-bundles/{id}` | Delete (409 while an upstream group names it) |
| GET | `/ws` | WebSocket: server-push events |

A subsystem takes an `https` object (`enabled`, `certificateId`, `redirect` `off`/`301`/`308`, `hsts`), and an upstream group a `tls` object (`enabled`, `verify`, `serverName`, `ca`). Requests that carry a private key (upload, replace with a key, self-signed) need an `admin` token and a **protected channel** (the admin plane on loopback, or served over TLS).

Config responses are `{ "data": ..., "version": N }`, where `version` is the config
revision the response reflects (for a write, the one it created); deletes return
`{ "version": N }`.

Errors always look like:

```json
{ "error": { "code": "NOT_FOUND", "message": "subsystem 42 not found", "status": 404 } }
```

Validation failures (400) add a `fields` array naming each bad input
(camelCase), and duplicates are `409`:

```json
{ "error": { "code": "VALIDATION_FAILED", "message": "validation failed", "status": 400,
             "fields": [{ "field": "prefix", "message": "must start with '/'" }] } }
```

WebSocket events are JSON discriminated by `type`:

```json
{ "type": "traffic",  "data": { "total": { "requestsPerSec": 12.5, "errorRate": 0.02, "...": "..." }, "subsystems": ["..."] } }
{ "type": "configChanged", "version": 7, "summary": "route created: /api in shop", "timestampMs": 0 }
{ "type": "activity", "kind": "config", "message": "route created: /api in shop", "timestampMs": 0 }
{ "type": "certState", "change": "uploaded", "certificate": "shop", "timestampMs": 0 }
{ "type": "hello",    "message": "Connected to Fast Proxy v0.1.0", "timestampMs": 0 }
```

## Maintenance mode

Take a whole proxy, one subsystem or one route out of service **temporarily**,
with an explanation, instead of disabling it (a disabled subsystem answers a bare
503 with no expiry). A request in a maintenance scope gets a **503** with
`Retry-After`, `Cache-Control: no-store` and a page (HTML, plain text or JSON,
chosen by `Accept`); it **never reaches an upstream**, consumes no retry budget
and touches no endpoint's health. Unknown hosts are still 404, plain HTTP still
redirects to HTTPS, and a WebSocket handshake gets the 503 (connections already
upgraded stay open). Health checks keep running, so the group is ready the moment
maintenance ends, and the admin plane is never in maintenance.

```sh
# the whole proxy for 30 minutes, with a custom message and a way in for you
curl -X PUT -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' \
  -d '{"message":"Upgrading the database","ttlSecs":1800,
       "bypass":{"cidrs":["10.0.0.0/8"],"header":"X-Maintenance-Pass"}}' \
  http://127.0.0.1:9000/api/maintenance/global
# -> {"data":{...},"version":N,"bypassSecret":"<shown once>"}

curl -X DELETE -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9000/api/maintenance/global
```

- **Scopes** are global, subsystem and route; any applicable scope that is on and
  does not let the request through answers it (widest first). A bypass belongs to
  its scope: passing a route's scope does not pass a subsystem-wide one.
- **`ttlSecs`** (up to 30 days) ends it by itself: the clock is checked on every
  request, so the end is exact, and a sweeper then clears the stored row. A
  restart during maintenance stays in maintenance.
- **Bypass** lets chosen source networks (`bypass.cidrs`) or requests carrying a
  secret header (`bypass.header`; the secret is generated and returned once if
  you do not give one of at least 16 characters) through, so you can verify a
  deploy before reopening. A request that passed carries
  `X-Maintenance-Bypass: <scope>`. The source address is the peer's, or the
  client behind a `trusted_proxies` peer, never a forged `X-Forwarded-For`.
- **`message`**, `body` and `contentType` (`text/html`, `text/plain`,
  `application/json`, up to 16 KiB) shape the answer; the built-in page escapes
  the message. `retryAfterSecs` overrides the value derived from the TTL (120 s
  when there is none).
- A **rollback never changes maintenance**, and maintenance is not part of the
  stored configuration. `POST /api/resolve` (optionally with `clientIp`) shows
  which scopes apply to a host and what each would do.
- To take a *backend* out of rotation use an endpoint's `draining`/`disabled`
  state instead; maintenance is for a *site*.

## TLS

TLS is **off by default** (no listener, no cost). Switch it on for the process with
`--tls` or `[data_plane.tls] enabled = true`; that starts one HTTPS listener
(default `0.0.0.0:8443`) beside the plain ones. Everything else is live and per
subsystem, in the admin UI or API:

- A subsystem opts in with `https.enabled`. It then has a certificate before its
  first request: the one you uploaded and attached, or a self-signed development
  certificate (`dev-<name>`) generated for its host names, which follows host
  changes and is regenerated before it expires. An uploaded certificate is never
  replaced automatically. Uploading checks that the key matches the certificate,
  that it is not expired and (when attaching) that it covers at least one of the
  subsystem's hosts.
- The certificate is chosen by SNI. A name nobody owns, or a subsystem without
  HTTPS, gets a throwaway placeholder certificate and then a 404 or 421
  (`reject_unknown_sni` fails the handshake instead).
- `https.redirect` (`308` or `301`) sends plain-HTTP requests to HTTPS and
  `https.hsts` adds `Strict-Transport-Security`; both apply only while TLS is on
  for the process.
- An upstream group's `tls` object makes the proxy talk TLS to its endpoints,
  verified against a named, uploaded CA bundle (or the OS trust store).

Walkthrough: [docs/getting-started.md](docs/getting-started.md) section 16.

### Running several instances

Several instances can share one database (a local SQLite file; a network database
is a later project) and the same **master key** (set `FAST_PROXY_MASTER_KEY`, or
copy the same `master.key`). Each polls the database every
`data_plane.config_poll_interval` (5 s) and, when another instance wrote, reloads
the configuration and certificates and swaps them in: a change made through one
instance is live on the others within a few seconds, and a renewal is safe in the
window because both certificates are valid. What stays **local to each instance**:
endpoint health and ejection, in-flight counts and TLS session tickets (so
resumption works within one instance). An instance started with a different
master key reports `key mismatch`, serves plain HTTP and the certificates it can
read, and never serves the ones it cannot.

## Observability

- **Metrics** at `GET /metrics` on the admin listener: `fastproxy_requests_total`
  (by subsystem, route and status class), `fastproxy_request_duration_seconds`,
  `fastproxy_upstream_errors_total` (connect, timeout, other),
  `fastproxy_active_connections`, `fastproxy_active_requests`,
  `fastproxy_config_version` and `fastproxy_config_reloads_total`; and per
  endpoint `fastproxy_upstream_requests_total` (by outcome),
  `fastproxy_upstream_endpoint_healthy`, `fastproxy_upstream_endpoint_in_flight`,
  `fastproxy_endpoint_ejections_total` and `fastproxy_health_checks_total`; per
  group `fastproxy_upstream_retries_total`, `fastproxy_retry_budget_exhausted_total`
  and `fastproxy_upstream_panic_total`; and `fastproxy_requests_cancelled_total`.
  Maintenance adds `fastproxy_maintenance_active{scope,subsystem,route}` (1 while
  on), `fastproxy_maintenance_responses_total{scope,subsystem,route}` and
  `fastproxy_maintenance_bypass_total{scope,subsystem}`; the dashboard's error
  rate does not count maintenance answers, which are intended.
  The admin plane adds `fastproxy_admin_requests_total{result}` (`ok`,
  `unauthenticated`, `forbidden`, `throttled`, `host_refused`) and
  `fastproxy_audit_entries_total{outcome}` (`ok`, `denied`, `failed`): a rise in
  `unauthenticated` or `throttled` is someone guessing tokens.
  TLS adds `fastproxy_tls_handshakes_total` (by listener, result and version),
  `fastproxy_tls_cert_expiry_seconds{cert,source}` and
  `fastproxy_tls_cert_unreadable{cert}` (labels are certificate names you chose),
  `fastproxy_cert_cache_total`, `fastproxy_config_sync_total`, and
  `fastproxy_upstream_tls{subsystem,upstream,verify}` (a group with
  `verify="false"` stands out).
  Label values come only from your configuration (names and endpoint
  addresses); unmatched traffic is `_unmatched`/`_none`. Series of removed
  endpoints are deleted.
- **Access log**, one line per request on both planes, to the console (tracing
  target `access`) and optionally a file. Proxied requests record subsystem,
  route, upstream (the endpoint that answered), attempts, request id, upstream and
  total time, bytes sent, the client protocol (`proto`: `http/1.1` or `h2`) and the
  TLS version (`tls`, `-` on plain HTTP). A request whose client left first is logged as 499. The file
  is written off the request path; the console line is the expensive one (see
  [docs/benchmarks.md](docs/benchmarks.md): about a quarter of throughput), so
  filter it with `--log-level "info,access=off"` when throughput matters.
- **Performance**: [docs/benchmarks.md](docs/benchmarks.md) has the baseline
  (about 25 us added per request at p50 on one connection), the cost of endpoint
  selection, the numbers per algorithm with 3 and 16 endpoints, config reload
  cost, and how to reproduce all of it.

## Security notes

- **Everything is audited.** Each committed change writes an audit entry in the
  same transaction as the change and its revision, naming who (the token's name
  and role), from where, what and the revision it created. The log also records
  what changed nothing: authentication failures (with the reason, and whether that
  failure tripped the throttle), refusals (`access.denied`), a foreign `Host`, a
  write the server rejected (400 or 409), and a token's first use. Entries hold
  names, ids and a short reason, never a body, a token, a ticket, a key or a PEM.
  Entries for unauthenticated requests are capped at 60 a minute for the whole
  process, so a flood cannot grow the log; the log is pruned to 90 days and
  200,000 rows.
- **The admin plane needs a bearer token.** Every API call and the WebSocket
  need one (only `GET /api/health` is open). The first start prints a bootstrap
  admin token once; `POST /api/tokens` creates more, with the role `admin`
  (everything) or `viewer` (read-only), and `DELETE /api/tokens/{id}` revokes
  one at once. Only a hash is stored, and the last admin token cannot be
  revoked. A `Host` header that is not an allowed admin name is refused (DNS
  rebinding), ten failed attempts from one address are throttled, and
  responses carry a strict `Content-Security-Policy`. The plane still binds to
  `127.0.0.1` by default. Leaving loopback needs **admin TLS**
  (`admin.tls.enabled`): the plane then serves HTTPS with a certificate stored in
  the database (a self-signed `admin` one is generated the first time; browsers
  warn about it until you upload your own of the same name), and the plain port
  is not open. The only way to leave loopback without TLS is the explicit,
  warned `admin.allow_insecure_remote = true`, which sends tokens in clear text.
  A reverse proxy in front makes every request look local to the admin plane, so
  it needs its own access control.
- **Private keys are write-only.** No endpoint, log line, metric, event, error or
  revision ever contains one; they are stored encrypted (AES-256-GCM) under a master
  key that lives outside the database, and requests that carry a key need an `admin`
  token over a protected channel (the admin plane on loopback, or served over TLS). See
  [docs/security-review-m3.md](docs/security-review-m3.md).
- **Back up the master key separately from the database.** The database alone does
  not expose the keys, and cannot be used without the master key: if the key is
  lost the stored private keys are unreadable and the certificates must be
  uploaded again (development certificates regenerate by themselves). Keep the
  `master.key` file (or `FAST_PROXY_MASTER_KEY`) somewhere other than the
  database's backups.
- HSTS is remembered by browsers for its whole lifetime: turn it on only when HTTPS
  works for every host of the subsystem.
- Client-supplied `X-Forwarded-*` headers are overwritten unless the peer is in
  `data_plane.trusted_proxies`.
- Config errors never echo file paths or storage details to API clients; the
  detail goes to the log.

## Project structure

```
├── docs/                     ROADMAP.md · M1-PLAN.md · M2-PLAN.md · M3-PLAN.md · getting-started.md · benchmarks.md · security-review-m3.md · adr/
├── scripts/                  bench-m2.sh · bench-m3.sh (the benchmark runs behind docs/benchmarks.md)
├── config.toml               process settings (listeners, admin, logging) + UI
├── src/
│   ├── main.rs               CLI entry + logging
│   ├── cli.rs                flags and how they override the config file
│   ├── config/               TOML config model and validation
│   └── startup.rs            assembles store, config service, data plane, admin plane
├── tests/                    end-to-end tests (the real binary: M1-M3 exit criteria, reload under load, two processes on one database; in process: TLS, HTTP/2, redirect, upstream TLS, live certificate replacement)
├── crates/
│   ├── fast-proxy-model/     validated domain types, group policies and the Config tree (no I/O)
│   ├── fast-proxy-balancing/ endpoint runtime state, selection algorithms, passive health (no I/O)
│   ├── fast-proxy-routing/   Config -> Snapshot compiler (carries runtime state), matchers (no I/O)
│   ├── fast-proxy-tls/       certificates: PEM validation, the key vault (AES-256-GCM), self-signed generator, certificate cache, rustls configurations
│   ├── fast-proxy-test-pki/  test support only: an in-memory CA and TLS test clients
│   ├── fast-proxy-store/     SQLite storage (certificates are opaque ciphertext here), transactional writes, revisions, rollback
│   ├── fast-proxy-config-service/  the only writer: validate, persist, swap snapshot
│   ├── fast-proxy-dataplane/ hyper listeners, request pipeline, endpoint selection, retries
│   ├── fast-proxy-health/    active health probes, reconciled with the live configuration
│   ├── fast-proxy-telemetry/ Prometheus metrics and the access log
│   └── fast-proxy-admin/     Axum admin plane: api/ · ws · events · error · embedded SPA
└── ui/
    ├── public/favicon.svg    the gear-and-bolt mark
    └── src/
        ├── lib/              typed API client (http, types, api), validation, navigation
        ├── icons/            the icon pack (add icons here)
        ├── context/          Theme · Toast · Config · Live (WebSocket)
        ├── components/       shell, forms/, subsystem/ sections, dialogs, chart, feed
        ├── pages/            Dashboard · Subsystems · SubsystemDetail · Revisions · Resolver · Settings · 404
        └── styles/           index.css (Tailwind v4 entry + theme tokens)
```

Dependencies point one way: `model` <- `tls`, `store`, `balancing`; `tls` <- `routing`; `balancing` <-
`routing`; `routing` + `balancing` + `telemetry` + `tls` <- `dataplane`, `health`;
`store` + `routing` + `balancing` + `telemetry` + `tls` <- `config-service`;
`config-service` <- `health`, `admin`; everything <- the binary. The data plane
never depends on the store.

## Extending

**New endpoint** - add a handler under `crates/fast-proxy-admin/src/api/`
returning `AppResult<Json<T>>`, register it in
`crates/fast-proxy-admin/src/router.rs`, add a typed function in
`ui/src/lib/api.ts` (and its types in `ui/src/lib/types.ts`). Writes must go
through `ConfigService::apply`.

**New page** - create `ui/src/pages/X.tsx`, add a route in `ui/src/router.tsx`, a
nav item in `components/Sidebar.tsx`, and a title in `lib/navigation.ts`.

**New server-push event** - add a variant to `Event` in
`crates/fast-proxy-admin/src/events.rs`, broadcast it via `state.broadcast(...)`,
handle its `type` in `ui/src/context/LiveContext.tsx`.

**Theming** - the design tokens live at the top of `ui/src/styles/index.css`
(one block per mode, registered as Tailwind colors via `@theme inline`).
Change the accent or surfaces there; every `bg-surface` / `text-ink` /
`bg-accent` utility picks the new values up in both light and dark mode.

## Testing

```sh
cargo test                       # unit tests per crate + end-to-end tests in tests/
cd ui && npm test                # frontend: Vitest + Testing Library
cargo llvm-cov --workspace --summary-only   # coverage (needs cargo-llvm-cov)
```

- `tests/exit_criteria.rs` runs the real binary: start from an empty database,
  configure it over the admin API, route traffic, edit live, roll back, restart,
  shut down gracefully (the signal tests run on Unix only).
- `tests/reload_under_load.rs` hammers the proxy with keep-alive clients while
  hundreds of config writes land, and checks that no request fails and no
  request is ever routed with a stale configuration.
- `tests/upstream_groups.rs` is the M2 exit criteria against three backends that
  can be killed, revived, slowed and made to fail: distribution per algorithm,
  killing one under load with no client errors, automatic return after recovery
  (passive and active), drain and disable (surviving a restart), and fail open
  versus fail closed.
- `tests/tls.rs`, `tls_http2.rs`, `tls_redirect.rs`, `tls_upstream.rs` and
  `tls_live_replacement.rs` run the real data plane and config service in one
  process with real TLS clients; `tls_exit_criteria.rs` and `cluster.rs` run the
  real binary (two processes on one database for the latter). Certificates come from an in-memory CA and are
  sent as PEM strings, so no files are involved.
- `tests/reload_under_load.rs` also changes endpoints (drain, disable, add,
  remove) under load and checks that no request starts on an endpoint after the
  write that took it out returned.
- `tests/operations.rs` is the M5 exit criteria against the real binary with
  authentication on: each maintenance scope closes only what it covers and nothing
  reaches the upstream, a bypass cannot be forged, health checks keep running, the audit
  trail names who did what (refusals included), a token can be made, used and revoked,
  and a flood of bad tokens is throttled. `tests/maintenance_under_load.rs` toggles
  maintenance on every scope under traffic (no failed request, the sharp rule after each
  toggle, and the backend sees exactly the requests answered 200),
  `tests/secret_hygiene.rs` scans every file, response, event, metric and log line for
  any secret, and `tests/cluster_operations.rs` runs two processes with different master
  keys on one database (`admin_auth.rs`, `admin_tls.rs`, `maintenance.rs` and `cli.rs`
  cover the rest). `docs/security-review-m5.md` records the review.
- Timing measurements are `#[ignore]`d tests, run in release mode (see
  [docs/benchmarks.md](docs/benchmarks.md)).
- `cargo clippy --all-targets -- -D warnings` and `cargo fmt --all -- --check`
  run in CI, which also tests on Linux, macOS and Windows.

## CI & releases

- **CI** (`.github/workflows/ci.yml`) runs on every push to `main` and every
  pull request: frontend test + build, `cargo fmt`/`clippy`, and `cargo test` +
  `cargo build --release` on Linux, macOS, and Windows.
- **Prepare Release** (`.github/workflows/release.yml`) is manual
  (Actions -> Prepare Release -> Run workflow, from `main`) and takes a `bump`
  input of `patch`, `minor`, or `major`. It updates `Cargo.toml`, `Cargo.lock`,
  `ui/package.json`, and `ui/package-lock.json`, then opens a `release/vX.Y.Z`
  pull request labeled `release`. The workflow fails before making changes if
  the release token is missing, the release branch already exists, the tag
  already exists, or a release PR for that version is already open.
- **Publish Release** (`.github/workflows/publish-release.yml`) runs on pushes
  to `main`, but publishes only when the pushed commit is the merged commit of a
  PR targeting `main` from a `release/` branch with the `release` label. It
  validates that all version files agree, builds binaries from the exact merge
  SHA, creates an annotated `vX.Y.Z` tag for that SHA, and creates or updates
  the GitHub Release. Reruns are safe when the existing tag already points to
  the same SHA; a tag pointing anywhere else fails the workflow. No release
  workflow commits directly to protected `main`.

### Release bot token

Create a repository secret named `RELEASE_BOT_TOKEN` for the Prepare Release
workflow. Use either a fine-grained personal access token or a GitHub App token
scoped only to this repository, with these repository permissions:

- Contents: read/write
- Pull requests: read/write
- Issues: read/write

The token is used only to push the `release/vX.Y.Z` branch, create the release
PR, and apply the `release` label.

## License

[MIT](LICENSE)
