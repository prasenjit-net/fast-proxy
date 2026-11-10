# Benchmarks

The M1 baseline: throughput and latency of a request through the proxy
compared with the same request sent straight to the backend. It is
**informational, not a gate**, and meant to be re-run to spot regressions and
to guide later work (for example caching metric label handles, load balancing).

## What was measured

| | |
|---|---|
| Hardware | Apple M4, 10 cores, 16 GB RAM |
| OS | macOS (Darwin 25.5.0) |
| Toolchain | rustc 1.98.0, `--release` profile (`lto = true`, `codegen-units = 1`) |
| Load generator | [`oha`](https://github.com/hatoo/oha) 1.16.0 |
| Code | commit `fa6981d` plus the benchmark helper and tests in this change |
| Traffic | HTTP/1.1 keep-alive over loopback, `GET /`, 2-byte response body |
| Run | 8 s per data point after a 2 s warm-up at 64 connections |

`oha`, the backend and the proxy all ran **on the same machine**, so they
compete for the same 10 cores. That matters for reading the numbers (see
below).

## Results

Throughput is requests per second; latency is per request, measured by the
client. Every request succeeded in every run (about 12 million in total).

| Connections | Direct rps | Proxy rps (access log off) | Proxy rps (log to file) | Proxy rps (default console log) |
|---:|---:|---:|---:|---:|
| 1 | 34,949 | 18,339 | 16,398 | 15,305 |
| 16 | 182,897 | 81,575 | 82,460 | 69,290 |
| 64 | 212,450 | 97,298 | 96,000 | 72,032 |
| 256 | 215,820 | 102,548 | 99,167 | 73,212 |

| Connections | Direct p50 / p99 | Proxy (log off) p50 / p99 | Proxy (default log) p50 / p99 |
|---:|---:|---:|---:|
| 1 | 0.028 / 0.044 ms | 0.053 / 0.069 ms | 0.063 / 0.092 ms |
| 16 | 0.084 / 0.154 ms | 0.181 / 0.378 ms | 0.205 / 0.581 ms |
| 64 | 0.290 / 0.608 ms | 0.636 / 1.171 ms | 0.858 / 1.632 ms |
| 256 | 1.144 / 2.476 ms | 2.400 / 4.865 ms | 3.381 / 6.803 ms |

### What the numbers say

- **Added latency for one connection is about 25 µs at p50** (0.028 to
  0.053 ms) with the access log off, and about 35 µs with the default logging.
  That is the cost of one extra hop: parsing, routing, forwarding, and the
  response accounting for metrics and the log.
- **Throughput through the proxy is under half of direct** at 16 or more
  connections (about 100k versus 215k rps). Under saturation each request is
  served twice (client to proxy, proxy to backend), on a CPU shared with the
  load generator and the backend, so a ratio near 50% is expected and is not a
  measure of the proxy's ceiling.
- **The default console access line costs about a quarter of throughput**
  (about 72k versus 97k rps at 64 connections). Writing the same lines to a
  **file** is nearly free (within noise of logging off), because the file is
  written by a background task. If throughput matters, filter the console line
  (`--log-level "info,access=off"`) and keep the file.
- The latency at 256 connections is queueing, not per-request work: with the
  CPU saturated, p50 grows with the number of requests in flight.

### Caveats

- The direct baseline is **bounded by `oha` and the backend sharing the CPU**,
  not by the backend. It understates what a real backend could do and
  overstates the proxy's relative cost. A fair absolute-capacity figure needs
  the load generator on another machine.
- Tiny responses and loopback make per-request overhead dominate. Large
  bodies, real network latency, and slow backends will shrink the proxy's share.
- One run per data point, on a laptop-class machine with other processes. Treat
  differences under about 10% as noise. Maximum latencies (outliers of tens of
  milliseconds) are noisy for the same reason; p99 was steadier.
- Absolute numbers depend on the load generator: a first pass with a small
  in-house generator gave the same picture (about 23 µs added at one
  connection, the same ordering of logging variants, throughputs within about
  10% of the above), but single-connection rates differed because tools do
  different per-request work. Compare runs made with the same tool.
- Not measured: TLS (M3), HTTP/2, many subsystems and routes, large header
  sets, WebSocket throughput, and memory use.

### Reading `oha`'s output

`oha` prints small latencies in microseconds and larger ones in milliseconds;
check the unit before comparing. At the end of the time limit it cuts off the
requests that are still in flight, one per connection, and lists them as
`aborted`; those are not failures.

## Reproducing

Build the release binary and the backend helper (an example in the data plane
crate, not part of the product):

```sh
cargo build --release -p fast-proxy
cargo build --release -p fast-proxy-dataplane --examples
```

Start a fast backend and the proxy, configure one subsystem through the admin
API, then drive both directly and through the proxy:

```sh
# backend: fixed 2-byte responses
target/release/examples/bench_backend 127.0.0.1:3001 2 &

# proxy, with the console access line filtered out
cat > bench.toml <<'TOML'
[data_plane]
state_dir = "./bench-state"
[[data_plane.listener]]
name = "http"
listen = "127.0.0.1:8000"
[admin]
listen = "127.0.0.1:9000"
TOML
target/release/fast-proxy --config bench.toml --log-level "info,access=off" &

A=http://127.0.0.1:9000/api; J='Content-Type: application/json'
curl -s -H "$J" -d '{"name":"bench"}' $A/subsystems
curl -s -H "$J" -d '{"pattern":"bench.example.com"}' $A/subsystems/1/hosts
curl -s -H "$J" -d '{"name":"backend","addr":"127.0.0.1:3001"}' $A/subsystems/1/upstreams
curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes

# direct, then through the proxy (the Host header selects the subsystem)
oha -z 8s -c 64 --no-tui http://127.0.0.1:3001/
oha -z 8s -c 64 --no-tui -H "Host: bench.example.com" http://127.0.0.1:8000/
```

To compare logging, add `access_log = "access.log"` under a `[logging]` section
(file), or drop the `access=off` filter (default console line). Vary `-c` for
the connection counts above.

## Endpoint selection cost (M2, task B8)

Cost of `Balancer::select` alone (no I/O), all endpoints eligible, release
build, Apple silicon laptop. Budget from the M2 plan: under 200 ns at three
endpoints, a few microseconds at 64, no allocation.

```sh
cargo test -p fast-proxy-balancing --release --test selection_cost -- --ignored --nocapture
```

| Algorithm | 3 endpoints | 64 endpoints |
|---|---|---|
| round_robin | 11.8 ns | 63.3 ns |
| weighted_round_robin | 4.3 ns | 61.2 ns |
| least_connections | 4.4 ns | 106.0 ns |
| p2c_ewma | 16.6 ns | 85.3 ns |
| consistent_hash | 6.5 ns | 12.0 ns |

All are well inside the budget. Round robin and p2c count and index the
eligible endpoints in a second pass, which is why they grow with group size;
that is fine at the 64-endpoint cap.

## M2 results: groups, algorithms and reload cost

Re-run of the procedure above at the end of M2 (same machine and tools; the
proxy ran with `--log-level "info,access=off"`; each point is 6 s after a 2 s
warm-up; the direct reference was measured in the same run). Retries, passive
health and endpoint selection are always on in these runs; the groups use
default policies and no active checks. Every request succeeded in every run.
The runs came from `scripts/bench-m2.sh`, which builds the release binaries,
starts 16 `bench_backend` instances and configures the proxy through the admin
API.

### One endpoint against the M1 baseline

The cost of everything M2 added to the request path for a group of one.

| Connections | Direct rps | M1 proxy rps | M2 proxy rps | Change | M2 p50 / p99 |
|---:|---:|---:|---:|---:|---:|
| 1 | 35,210 | 18,339 | 17,793 | -3.0% | 0.054 / 0.072 ms |
| 16 | 179,573 | 81,575 | 85,126 | +4.4% | 0.181 / 0.307 ms |
| 64 | 208,465 | 97,298 | 97,920 | +0.6% | 0.634 / 1.162 ms |

The added latency at one connection is about 27 µs at p50 (0.027 ms direct,
0.054 ms through the proxy) against about 25 µs in M1. The differences are
within the run-to-run noise noted below (about 10%), so a single-endpoint group
is **not measurably slower than M1**; the plan's budget was 5%.

### Three and 16 endpoints, per algorithm (64 connections)

| Algorithm | 3 endpoints rps | p50 | 16 endpoints rps | p50 |
|---|---:|---:|---:|---:|
| round_robin | 99,007 | 0.629 ms | 96,607 | 0.647 ms |
| weighted_round_robin | 98,707 | 0.628 ms | 96,124 | 0.646 ms |
| least_connections | 99,036 | 0.632 ms | 93,475 | 0.657 ms |
| p2c_ewma | 99,034 | 0.630 ms | 96,179 | 0.641 ms |
| consistent_hash | 100,019 | 0.624 ms | 96,972 | 0.641 ms |

Selecting among three endpoints costs nothing visible: throughput equals the
single-endpoint figure (about 98k rps). With 16 endpoints it is 1% to 5% lower
(least connections is the lowest at 93.5k, since it looks at every endpoint's
load); that is about what the selection timings above predict, and it is small
next to the cost of the request itself. Consistent hash is the cheapest at both
sizes because a lookup in the precomputed ring does not scan the endpoints.

### Reload cost with a large configuration

`cargo test -p fast-proxy-config-service --release --test reload_cost -- --ignored --nocapture`
builds 200 subsystems with 4,000 routes and 600 endpoints, then measures:

| What | Time |
|---|---:|
| Compile, carrying every endpoint's runtime state over (median of 20) | 0.64 ms |
| Compile from scratch | 0.63 ms |
| A whole change, a small edit: mutate, compile as the veto, store the full-configuration revision, swap (median of 10) | 7.5 ms |

A change to a configuration this size takes milliseconds, almost all of it the
revision write (the whole configuration is stored per change); compiling,
including building consistent-hash rings and carrying runtime state, is well
under a millisecond. In-flight requests are not affected: they keep the
snapshot they started with.

### Caveats

The M1 caveats apply: one machine shared by the load generator, backends and
proxy; one run per point; treat differences under about 10% as noise. The
throughput at 16 endpoints and the one-endpoint comparison in particular are
inside that band. Reproduce with the commands above, and run every throughput point with
`scripts/bench-m2.sh results.txt` (needs `oha`; it builds the release binaries,
starts the backends and configures the proxy itself).

## M3 results: TLS

Run with `scripts/bench-m3.sh <results-file>` (builds the release binaries, starts
a plain and a TLS `bench_backend`, and drives the real proxy with `oha`). Same
machine and tools as above; the proxy ran with `--log-level "info,access=off"`;
each point is 6 s after a 2 s warm-up. Every request succeeded in every run. The
proxy and `oha` share the CPU, as before. (Latencies in the script's output mix
microseconds and milliseconds because `oha` does; they are normalised here.)

### Plain HTTP must not slow down (M3 budget: within about 5% of M2)

| Connections | M2 rps | TLS off, rps | TLS on, plain HTTP, rps | TLS off vs M2 | TLS on vs off |
|---:|---:|---:|---:|---:|---:|
| 1 | 17,793 | 17,460 | 17,681 | -1.9% | +1.3% |
| 16 | 85,126 | 84,190 | 83,183 | -1.1% | -1.2% |
| 64 | 97,920 | 96,792 | 93,850 | -1.2% | -3.0% |

Everything M3 added to the request path (the redirect check, scheme handling,
certificate and TLS state in the snapshot) is not measurable with TLS off, and
turning TLS on costs plain-HTTP traffic at most 3%, inside the budget and the run
to run noise. The added latency at one connection is still about 27 µs at p50.

### HTTPS, keep-alive

| Connections | Plain HTTP rps | HTTPS HTTP/1.1 rps (p50 / p99) | HTTPS HTTP/2 rps (p50 / p99) |
|---:|---:|---:|---:|
| 1 | 17,681 | 16,799 (0.058 / 0.075 ms) | 15,216 (0.064 / 0.083 ms) |
| 16 | 83,183 | 79,037 (0.193 / 0.374 ms) | 72,653 (0.207 / 0.441 ms) |
| 64 | 93,850 | 91,216 (0.678 / 1.268 ms) | 80,028 (0.763 / 1.607 ms) |

With keep-alive the cost is mostly the symmetric crypto: HTTPS over HTTP/1.1 is
3% to 5% below plain HTTP at one and 16 connections and 3% at 64 (about 3 µs more
at p50 on one connection). HTTP/2 is 10% to 14% below HTTPS HTTP/1.1: the
framing, header compression and stream bookkeeping cost more CPU per request
than they save here, because the load generator and the proxy share the cores and
every response is two bytes. The case for HTTP/2 is fewer connections and
multiplexing for browsers, not raw loopback throughput.

### Handshakes

One new TCP and TLS connection per request (`oha --disable-keepalive`), so each
request pays a handshake:

| Connections | Requests/s | p50 | p99 |
|---:|---:|---:|---:|
| 1 | 1,091 | 0.87 ms | 1.4 ms |
| 16 | 6,349 | 2.2 ms | 8.0 ms |
| 64 | 6,591 | 8.6 ms | 26.5 ms |

About 6,600 new connections per second saturates the machine (the client does its
side of every handshake on the same cores). Session tickets are on at the server;
`oha` offers no switch for resumption, so a with/without-resumption comparison was
**not** measured and would need a purpose-built client.

### TLS to the upstream

HTTP to the proxy and TLS (verified against an uploaded CA, with a name override)
to the backend, keep-alive pool: 17,305 / 82,221 / 92,419 rps at 1 / 16 / 64
connections, within 2% to 3% of the same run with a plain upstream. The handshake
is paid once per pooled connection.

### Certificate writes and the cache

- A certificate write (generate a self-signed certificate, seal its key, store it,
  compile the snapshot, swap): median **0.4 ms**, p95 0.4 ms, flat from 1 to 60
  stored certificates.
- Rebuilding the certificate cache with the 256 certificates the limit allows:
  **2.4 ms** to decrypt and parse every one (this is also the startup cost), and
  **26 µs** when nothing changed, because unchanged entries are reused.
  (`cargo test -p fast-proxy-tls --release rebuilding_256 -- --nocapture`.)

Both are far inside the plan's budget (low milliseconds for a rebuild, well under
a second for a full parse), and neither stalls requests: the swap is one pointer
swap and the work happens off the request path.

### Upstream HTTP/2 (decision C4)

The plan left HTTP/2 to upstreams open until there were numbers. The data say
there is nothing to win on the upstream hop: pooled HTTP/1.1 keep-alive
connections to the upstream cost 1% to 3% (even over TLS), and HTTP/2 on the
client side is slower than HTTP/1.1 on loopback. **Decision: upstream connections
stay HTTP/1.1.** Revisit it only for a concrete need (an h2-only backend, gRPC).
