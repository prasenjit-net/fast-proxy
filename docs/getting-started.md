# Getting started

A copy-paste walkthrough that sets up two subsystems through the admin API and
sends traffic through the proxy. It follows the M1 exit criteria and uses
`curl` so every step is explicit.

**Prefer clicking?** Everything below can also be done in the admin UI at
http://127.0.0.1:9000 (or http://localhost:5173 with `npm run dev`):

- **Subsystems**: create a subsystem, then on its page add hosts, upstreams and
  routes. Forms check your input and show the server's messages next to the
  fields; deletes ask first and say what will be removed.
- **Revisions**: browse the history, view the configuration at any revision, and
  roll back.
- **Resolver**: try a host and path and see the subsystem, route and the exact
  upstream URL, or why nothing matched.
- **Dashboard**: live requests/s, error rate and connections, by subsystem.

## 1. Start the proxy

```sh
cd ui && npm install && npm run build && cd ..   # once, for the embedded UI
cargo run                                        # uses ./config.toml
```

With the default config the **admin plane** is on `127.0.0.1:9000` and the
**data plane** (the proxy itself) is on `0.0.0.0:8000`. State, including the
SQLite database, lives in `./state/`. Delete that directory to start again from
an empty database.

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

# shop
curl -s -H "$J" -d '{"name":"shop"}' $A/subsystems
curl -s -H "$J" -d '{"pattern":"shop.example.com"}' $A/subsystems/1/hosts
curl -s -H "$J" -d '{"name":"backend","endpoints":[{"addr":"127.0.0.1:3001"}]}' $A/subsystems/1/upstreams
curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes

# blog
curl -s -H "$J" -d '{"name":"blog"}' $A/subsystems
curl -s -H "$J" -d '{"pattern":"blog.example.com"}' $A/subsystems/2/hosts
curl -s -H "$J" -d '{"name":"backend","endpoints":[{"addr":"127.0.0.1:3002"}]}' $A/subsystems/2/upstreams
curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":2}' $A/subsystems/2/routes
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
curl -s $A/subsystems          # list: host patterns, route counts
curl -s $A/subsystems/1        # detail: hosts, upstreams, routes (longest prefix first)
```

## 6. Edit live

Point the shop at the blog's backend, and watch the very next request change:

```sh
curl -s -X PUT -H "$J" -d '{"addr":"127.0.0.1:3002"}' $A/subsystems/1/upstreams/1/endpoints/1
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # hello from blog
```

Add a path-based route with a rewrite (here `/old/x` is sent to the backend as `/x`).
The new upstream gets id `3`:

```sh
curl -s -H "$J" -d '{"name":"legacy","endpoints":[{"addr":"127.0.0.1:3001"}]}' $A/subsystems/1/upstreams
curl -s -H "$J" -d '{"prefix":"/old","upstreamGroupId":3,"stripPrefix":true}' $A/subsystems/1/routes
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/old/index.html   # hello from shop
```

Switch a subsystem off (503) and on again:

```sh
curl -s -X PUT -H "$J" -d '{"name":"shop","enabled":false}' $A/subsystems/1
curl -si -H 'Host: shop.example.com' http://127.0.0.1:8000/ | head -1      # 503
curl -s -X PUT -H "$J" -d '{"name":"shop","enabled":true}' $A/subsystems/1
```

## 7. Dry-run a request with the resolver

`POST /api/resolve` performs the same lookup the proxy does and tells you what
would happen, including the exact upstream URL, without sending anything:

```sh
curl -s -H "$J" -d '{"host":"shop.example.com","path":"/old/index.html?x=1"}' $A/resolve
# matched: true, route.prefix "/old", upstreamUrl "http://127.0.0.1:3001/index.html?x=1"
curl -s -H "$J" -d '{"host":"nope.example.com","path":"/"}' $A/resolve
# matched: false, reason: "unknown_host"   (or "subsystem_disabled" / "no_route")
```

## 8. History and rollback

Every successful change is a numbered revision holding the full configuration
at that moment:

```sh
curl -s "$A/revisions?limit=5"       # newest first; pass before=<nextBefore> for older ones
curl -s $A/revisions/4               # the whole configuration as of revision 4
```

Undo a mistake by rolling back, which re-applies an earlier revision as a **new**
revision (nothing is rewritten). Ids are kept:

```sh
curl -s -X DELETE $A/subsystems/1                        # oops
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # 404
curl -s -X POST $A/revisions/4/rollback                  # restoredFrom: 4
curl -s -H 'Host: shop.example.com' http://127.0.0.1:8000/index.html   # works again
```

The last 100 revisions are kept.

## 9. What is running

```sh
curl -s $A/status      # listeners, database path, subsystem counts, uptime, live config version
```

## 10. Metrics, access log, and live traffic

Prometheus metrics are served on the admin listener, and every proxied request
is written to the access log (console, and the file named by
`logging.access_log` if set):

```sh
curl -s http://127.0.0.1:9000/metrics | grep fastproxy_
# fastproxy_requests_total{route="/",status_class="2xx",subsystem="shop"} 6
# fastproxy_request_duration_seconds_bucket / _sum / _count
# fastproxy_upstream_errors_total{kind="connect|timeout|other",subsystem="shop"}
# fastproxy_active_connections, fastproxy_active_requests
# fastproxy_config_version, fastproxy_config_reloads_total{outcome="applied|rejected"}
```

Label values come only from your configuration (subsystem names and route
prefixes); traffic for an unknown host is counted as `subsystem="_unmatched"`,
and requests with no route as `route="_none"`. An access-log line looks like:

```
127.0.0.1:50110 - [2026-11-09T19:56:46.075Z] "GET /index.html HTTP/1.1" 200 plane=data subsystem=shop route=/ upstream=127.0.0.1:3001 attempts=1 request_id=f9cce2d6… upstream_ms=3.0 bytes_out=16 total_ms=3.2
```

The dashboard's numbers come from the same counters, sampled every 2 seconds:

```sh
curl -s http://127.0.0.1:9000/api/traffic      # requests/s, error rate, connections, per subsystem
```

## 11. WebSockets and other protocol upgrades

Requests with `Connection: Upgrade` / `Upgrade: websocket` are passed through:
if the upstream answers `101`, the proxy relays bytes both ways until either
side closes. Point a subsystem at any WebSocket server and connect to the
proxy with the subsystem's `Host`; no extra configuration is needed.

## 12. Validation errors name every bad field

```sh
curl -s -H "$J" -d '{"prefix":"no-slash","upstreamGroupId":1,"connectTimeoutMs":5}' \
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

## Not available yet

Authentication for the admin plane, TLS, multiple endpoints per upstream and load
balancing are later milestones (see [ROADMAP.md](ROADMAP.md)).
