# Getting started

A copy-paste walkthrough that sets up two subsystems through the admin API and
sends traffic through the proxy, then (from section 15) puts several backends
behind one upstream, kills one, and drains another. It follows the M1 and M2
exit criteria and uses `curl` so every step is explicit.

This walkthrough also covers TLS (section 16).

**Prefer clicking?** Everything below can also be done in the admin UI at
http://127.0.0.1:9000 (or http://localhost:5173 with `npm run dev`):

- **Subsystems**: create a subsystem, then on its page add hosts, upstreams (each
  with its endpoints, load-balancing and health settings) and routes. Forms check your input and show the server's messages next to the
  fields; deletes ask first and say what will be removed.
- **Revisions**: browse the history, view the configuration at any revision, and
  roll back.
- **Resolver**: try a host and path and see the subsystem, route, the upstream's
  algorithm and every endpoint's URL and eligibility, or why nothing matched.
- **Dashboard**: live requests/s, error rate and connections by subsystem, and
  which upstream endpoints are out of rotation and why.

## 1. Start the proxy

```sh
cd ui && npm install && npm run build && cd ..   # once, for the embedded UI
cargo run                                        # uses ./config.toml
```

With the default config the **admin plane** is on `127.0.0.1:9000` and the
**data plane** (the proxy itself) is on `0.0.0.0:8000`. State, including the
SQLite database, lives in `./state/`. Delete that directory to start again from
an empty database.

**The admin plane needs a token.** On the first start the proxy prints one,
once, on the console:

```
admin token (shown once, store it now): fp_1.<secret>
```

Keep it in a variable for the rest of this walkthrough (a later start prints
nothing; create more tokens with `POST /api/tokens`). For throwaway local
experiments you can instead start with `--admin-auth off`, which is accepted only
while the admin plane listens on loopback and is warned about on every start.

```sh
export TOKEN=fp_1.<the secret printed above>
```

## 2. Start two backends

In two more terminals:

```sh
mkdir -p /tmp/shop /tmp/blog
echo "hello from shop" > /tmp/shop/index.html
echo "hello from blog" > /tmp/blog/index.html
(cd /tmp/shop && python3 -m http.server 3001 --bind 127.0.0.1)
(cd /tmp/blog && python3 -m http.server 3002 --bind 127.0.0.1)
```

## 3. Nothing is configured yet

```sh
curl -i -H 'Host: shop.example.com' http://127.0.0.1:8000/
# HTTP/1.1 404 Not Found ... unknown host
```

## 4. Create the two subsystems

A subsystem is one host name with its own upstreams and routes. Each call
prints the resource and the config `version` it created.

```sh
A=http://127.0.0.1:9000/api
J='Content-Type: application/json'
H="Authorization: Bearer $TOKEN"      # admin calls below carry this; calls to :8000 do not

# shop
curl -s -H "$H" -H "$J" -d '{"name":"shop"}' $A/subsystems
curl -s -H "$H" -H "$J" -d '{"pattern":"shop.example.com"}' $A/subsystems/1/hosts
curl -s -H "$H" -H "$J" -d '{"name":"backend","endpoints":[{"addr":"127.0.0.1:3001"}]}' $A/subsystems/1/upstreams
curl -s -H "$H" -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes

# blog
curl -s -H "$H" -H "$J" -d '{"name":"blog"}' $A/subsystems
curl -s -H "$H" -H "$J" -d '{"pattern":"blog.example.com"}' $A/subsystems/2/hosts
curl -s -H "$H" -H "$J" -d '{"name":"backend","endpoints":[{"addr":"127.0.0.1:3002"}]}' $A/subsystems/2/upstreams
curl -s -H "$H" -H "$J" -d '{"prefix":"/","upstreamGroupId":2}' $A/subsystems/2/routes
```

The ids above assume a fresh database; use the `id` values from the responses
otherwise. Subsystem ids and upstream ids are each numbered across the whole
database, not per subsystem, so the blog's upstream is `2`, and the next
upstream you create is `3`. A route may only use an upstream of its own
subsystem; anything else is rejected with a `400` naming `upstreamGroupId`.

## 5. Traffic is routed immediately

```sh
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # hello from shop
curl -s -H 'Host: blog.example.com' http://127.0.0.1:8000/index.html   # hello from blog
curl -si -H 'Host: other.example.com' http://127.0.0.1:8000/           # 404 unknown host
```

No restart was needed. Look at what is configured:

```sh
curl -s -H "$H" $A/subsystems          # list: host patterns, route counts
curl -s -H "$H" $A/subsystems/1        # detail: hosts, upstreams, routes (longest prefix first)
```

## 6. Edit live

Point the shop at the blog's backend, and watch the very next request change:

```sh
curl -s -H "$H" -X PUT -H "$J" -d '{"addr":"127.0.0.1:3002"}' $A/subsystems/1/upstreams/1/endpoints/1
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # hello from blog
```

Add a path-based route with a rewrite (here `/old/x` is sent to the backend as `/x`).
The new upstream gets id `3`:

```sh
curl -s -H "$H" -H "$J" -d '{"name":"legacy","endpoints":[{"addr":"127.0.0.1:3001"}]}' $A/subsystems/1/upstreams
curl -s -H "$H" -H "$J" -d '{"prefix":"/old","upstreamGroupId":3,"stripPrefix":true}' $A/subsystems/1/routes
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/old/index.html   # hello from shop
```

Switch a subsystem off (503) and on again:

```sh
curl -s -H "$H" -X PUT -H "$J" -d '{"name":"shop","enabled":false}' $A/subsystems/1
curl -si -H 'Host: shop.example.com' http://127.0.0.1:8000/ | head -1      # 503
curl -s -H "$H" -X PUT -H "$J" -d '{"name":"shop","enabled":true}' $A/subsystems/1
```

## 7. Dry-run a request with the resolver

`POST /api/resolve` performs the same lookup the proxy does and tells you what
would happen, including the exact URL for each endpoint of the upstream and whether it would
take a request right now, without sending anything:

```sh
curl -s -H "$H" -H "$J" -d '{"host":"shop.example.com","path":"/old/index.html?x=1"}' $A/resolve
# matched: true, route.prefix "/old", route.algorithm "round_robin",
# route.endpoints[0].upstreamUrl "http://127.0.0.1:3001/index.html?x=1", eligible: true
curl -s -H "$H" -H "$J" -d '{"host":"nope.example.com","path":"/"}' $A/resolve
# matched: false, reason: "unknown_host"   (or "subsystem_disabled" / "no_route")
```

## 8. History and rollback

Every successful change is a numbered revision holding the full configuration
at that moment:

```sh
curl -s -H "$H" "$A/revisions?limit=5"       # newest first; pass before=<nextBefore> for older ones
curl -s -H "$H" $A/revisions/4               # the whole configuration as of revision 4
```

Undo a mistake by rolling back, which re-applies an earlier revision as a **new**
revision (nothing is rewritten). Ids are kept:

```sh
curl -s -H "$H" -X DELETE $A/subsystems/1                        # oops
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # 404
curl -s -H "$H" -X POST $A/revisions/4/rollback                  # restoredFrom: 4
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # works again
```

The last 100 revisions are kept.

## 9. What is running

```sh
curl -s -H "$H" $A/status      # listeners, database path, subsystem counts, upstream health, uptime, live config version
```

## 10. Metrics, access log, and live traffic

Prometheus metrics are served on the admin listener, and every proxied request
is written to the access log (console, and the file named by
`logging.access_log` if set):

```sh
curl -s -H "$H" http://127.0.0.1:9000/metrics | grep fastproxy_
# fastproxy_requests_total{route="/",status_class="2xx",subsystem="shop"} 6
# fastproxy_request_duration_seconds_bucket / _sum / _count
# fastproxy_upstream_errors_total{kind="connect|timeout|other",subsystem="shop"}
# fastproxy_active_connections, fastproxy_active_requests
# fastproxy_config_version, fastproxy_config_reloads_total{outcome="applied|rejected"}
# fastproxy_upstream_requests_total{endpoint=..,outcome="success|bad_status|connect|timeout|other",..}
# fastproxy_upstream_retries_total{reason=..}, fastproxy_retry_budget_exhausted_total
# fastproxy_endpoint_ejections_total, fastproxy_health_checks_total{result="ok|fail"}
# fastproxy_upstream_endpoint_healthy, fastproxy_upstream_endpoint_in_flight
# fastproxy_upstream_panic_total, fastproxy_requests_cancelled_total
```

Label values come only from your configuration (subsystem, upstream and route
names and endpoint addresses); traffic for an unknown host is counted as `subsystem="_unmatched"`,
and requests with no route as `route="_none"`. An access-log line looks like:

```
127.0.0.1:50110 - [2026-11-09T19:56:46.075Z] "GET /index.html HTTP/1.1" 200 plane=data subsystem=shop route=/ upstream=127.0.0.1:3001 attempts=1 request_id=f9cce2d6… upstream_ms=3.0 bytes_out=16 proto=http/1.1 tls=- total_ms=3.2
```

`upstream` is the endpoint that answered (the last one tried) and `attempts` how
many were tried: 2 means the first failed and a retry succeeded. `proto` is the client protocol (`http/1.1` or `h2`) and `tls` the negotiated TLS
version on the HTTPS listener (`-` on plain HTTP). A request whose
client left before the response is logged with status `499` and counted in
`fastproxy_requests_cancelled_total`.

The dashboard's numbers come from the same counters, sampled every 2 seconds:

```sh
curl -s -H "$H" http://127.0.0.1:9000/api/traffic      # requests/s, error rate, connections, per subsystem
```

## 11. WebSockets and other protocol upgrades

Requests with `Connection: Upgrade` / `Upgrade: websocket` are passed through:
if the upstream answers `101`, the proxy relays bytes both ways until either
side closes. Point a subsystem at any WebSocket server and connect to the
proxy with the subsystem's `Host`; no extra configuration is needed.

## 12. Validation errors name every bad field

```sh
curl -s -H "$H" -H "$J" -d '{"prefix":"no-slash","upstreamGroupId":1,"connectTimeoutMs":5}' \
  $A/subsystems/1/routes
# {"error":{"code":"VALIDATION_FAILED", ..., "fields":[
#   {"field":"prefix", ...}, {"field":"connectTimeoutMs", ...}]}}
```

Other errors: `404` for something that does not exist, `409` for a duplicate
(subsystem name, host pattern, route prefix) or an upstream a route still uses.

## 13. Shutting down

Ctrl-C starts a graceful shutdown: the proxy stops accepting new connections,
closes idle ones, and lets requests that are already in flight (and open
WebSockets) finish for up to `data_plane.shutdown_grace` (30 seconds by
default). Anything still open after that is closed. The admin API stays up
until the data plane has drained.

## 14. The configuration survives a restart

Start the proxy again with `cargo run`. The same requests route the same way:
the configuration is in `./state/fast-proxy.db`.

## 15. Several endpoints in one upstream

An upstream is a group of endpoints behind one name. Start two more backends
(ports 3003 and 3004), then create a group of three for the shop:

```sh
mkdir -p /tmp/shop2 /tmp/shop3 && echo "hello from shop 2" > /tmp/shop2/index.html && echo "hello from shop 3" > /tmp/shop3/index.html
(cd /tmp/shop2 && python3 -m http.server 3003 --bind 127.0.0.1)    # terminal
(cd /tmp/shop3 && python3 -m http.server 3004 --bind 127.0.0.1)    # terminal

curl -s -H "$H" -H "$J" -d '{
  "name": "pool",
  "lb": { "algorithm": "round_robin" },
  "health": { "check": "http", "path": "/index.html", "intervalMs": 1000, "timeoutMs": 500 },
  "endpoints": [ {"addr":"127.0.0.1:3001"}, {"addr":"127.0.0.1:3003"}, {"addr":"127.0.0.1:3004"} ]
}' $A/subsystems/1/upstreams                                    # the new upstream gets id 4 (its endpoints 4, 5, 6)
curl -s -H "$H" -X PUT -H "$J" -d '{"prefix":"/","upstreamGroupId":4}' $A/subsystems/1/routes/1   # point "/" at the pool
for i in 1 2 3 4 5 6; do curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html; done
# hello from shop, shop 2, shop 3, shop, shop 2, shop 3
```

The other algorithms are `weighted_round_robin` (give endpoints a `weight`),
`least_connections`, `p2c_ewma` (prefers faster, less busy endpoints) and
`consistent_hash` (`"hashKey": "client_ip"`, `"header:x-user"` or `"cookie:sid"`).

**Kill one and watch it leave.** Stop the backend on 3003 (Ctrl-C) and keep
sending requests: none fail, because a failed connect is retried on another
endpoint, and after three failures in a row the proxy stops using the dead one.
The health checks notice too, with no traffic at all:

```sh
for i in $(seq 1 12); do curl -s -o /dev/null -w '%{http_code} ' -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html; done   # 200 x12
curl -s -H "$H" $A/subsystems/1/upstreams/4/status    # 127.0.0.1:3003 -> effective "ejected" or "unhealthy", with the reason
curl -s -H "$H" $A/status                             # upstreams: 2 of 3 healthy, and which one is out
```

**Watch it return.** Start the backend on 3003 again. Within a few seconds
(health checks, or the ejection expiring and one trial request succeeding) its
`effective` state is `healthy` and it takes traffic again. Nobody did anything.

**Drain one before taking it down.** Draining (or disabling) stops *new* requests
to an endpoint while the ones already on it finish; the state is saved and
survives a restart:

```sh
curl -s -H "$H" -X PUT -H "$J" -d '{"state":"draining"}' $A/subsystems/1/upstreams/4/endpoints/5/state
curl -s -H "$H" $A/subsystems/1/upstreams/4/status    # inFlight shows what is left; 0 means it is drained
curl -s -H "$H" -X PUT -H "$J" -d '{"state":"enabled"}'  $A/subsystems/1/upstreams/4/endpoints/5/state
```

Endpoints can also be added, changed (`weight`, `addr`) and removed one by one
(`POST/PUT/DELETE $A/subsystems/1/upstreams/4/endpoints[/ID]`); the last endpoint of a
group cannot be removed. If **every** endpoint is unhealthy the group keeps
serving anyway ("fail open", so a wrong health verdict is not a total outage),
or answers `503` if you set `"failurePolicy": "fail_closed"`. A draining or
disabled endpoint is never used, even then. Routes can tune retries with
`"retry": {"maxAttempts": 2, "idempotentOn5xx": true, "onTimeout": false}`.

## 16. TLS

Everything below uses `curl`, the admin API on `127.0.0.1:9000` and `openssl` to
make a test certificate. (The admin UI does the same: a subsystem's page has an
**HTTPS** section and there is a **Certificates** page.)

**Turn TLS on** for the process. It is off by default; this starts an HTTPS
listener on 8443 next to the plain one on 8000:

```sh
cargo run -- --tls          # or [data_plane.tls] enabled = true in config.toml
curl -s -H "$H" $A/tls              # enabled: true, listen "0.0.0.0:8443", vault "ok" or "not created"
```

**Enable HTTPS on a subsystem** and see its development certificate. A subsystem
without a certificate gets a self-signed one for its host names, generated in the
same write:

```sh
curl -s -H "$H" -X PUT -H "$J" -d '{"name":"shop","https":{"enabled":true}}' $A/subsystems/1
curl -sk --resolve shop.example.com:8443:127.0.0.1 https://shop.example.com:8443/index.html   # hello from shop
curl -s -H "$H" $A/certificates     # dev-shop, source "self_signed", usedBy [1]; no key is ever shown
```

(`-k` because a development certificate is not trusted by anyone.) Browsers warn
for the same reason: upload a real certificate for production.

**Upload a certificate and key** for the subsystem. Here a throwaway one made with
`openssl`; the key must be unencrypted (it is encrypted at rest here) and
requests that carry a key are accepted only from this machine:

```sh
openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=shop.example.com" \
  -addext "subjectAltName=DNS:shop.example.com" -keyout /tmp/shop.key -out /tmp/shop.crt 2>/dev/null
python3 - <<'PY' | curl -s -H "$H" -H "$J" -d @- $A/subsystems/1/certificate
import json; print(json.dumps({"name": "shop", "certPem": open("/tmp/shop.crt").read(), "keyPem": open("/tmp/shop.key").read()}))
PY
curl -s --cacert /tmp/shop.crt --resolve shop.example.com:8443:127.0.0.1 https://shop.example.com:8443/index.html
```

The upload is rejected, and nothing changes, if the key does not match, the
certificate is expired, or it covers none of the subsystem's hosts (the error names
the field, for example `keyPem`, and never echoes the PEM). The next handshake
after a **replace** (`PUT $A/certificates/ID` with a new `certPem` and `keyPem`)
serves the new certificate, with no restart and without touching connections that
are already open.

**SNI and HTTP/2.** Each subsystem is served with its own certificate, chosen by
the name the client sends; a name nobody owns gets a throwaway placeholder and a
404. HTTP/1.1 and HTTP/2 share the port:

```sh
curl -s --http2 --cacert /tmp/shop.crt --resolve shop.example.com:8443:127.0.0.1 -w '%{http_version}\n' -o /dev/null https://shop.example.com:8443/
# 2
```

A request whose `Host` belongs to a subsystem served by a **different**
certificate than the connection's gets **421 Misdirected Request** (hosts that
share a certificate are fine).

**Redirect and HSTS** are per subsystem and live. They act only while TLS is on:

```sh
curl -s -H "$H" -X PUT -H "$J" -d '{"name":"shop","https":{"enabled":true,"certificateId":CERT_ID,"redirect":"308","hsts":{"enabled":true,"maxAgeSecs":300}}}' $A/subsystems/1
curl -si -H 'Host: shop.example.com' http://127.0.0.1:8000/cart?x=1 | head -3   # 308, location: https://shop.example.com:8443/cart?x=1
```

Browsers remember HSTS for `maxAgeSecs`, so start with a short one.

**TLS to an upstream.** Upload the CA that signed the backend's certificate, then
mark the group TLS (`serverName` is needed when the endpoint is an IP address):

```sh
curl -s -H "$H" -H "$J" -d "{\"name\":\"internal\",\"pem\":$(python3 -c 'import json;print(json.dumps(open("/tmp/ca.pem").read()))')}" $A/ca-bundles
curl -s -H "$H" -X PUT -H "$J" -d '{"name":"backend","tls":{"enabled":true,"serverName":"backend.internal","ca":"internal"}}' $A/subsystems/1/upstreams/UPSTREAM_ID
```

A certificate the proxy does not trust is refused (a 502, counted as a connect
failure) and the group's health checks use the same TLS settings, so the endpoint
shows as unhealthy too. `"verify": false` skips verification: it is allowed, logged
as a warning, shown in the UI and marked in the `fastproxy_upstream_tls` metric.

**The master key.** The first time a private key is stored the proxy creates
`state/master.key` and encrypts every stored key with it. The database alone does
not contain the keys, so **back the key up separately from the database**; if it
is lost the keys are unreadable and certificates must be uploaded again. To run
several instances on one database give them all the same key
(`FAST_PROXY_MASTER_KEY`): a change on one reaches the others within a few seconds.

## Not available yet

Authentication for the admin plane, ACME certificates, maintenance mode, rate
limits and request sampling are later milestones (see [ROADMAP.md](ROADMAP.md)).
