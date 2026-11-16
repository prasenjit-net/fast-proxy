#!/bin/bash
# The M4 benchmark behind docs/benchmarks.md: the cost of ACME on the data plane.
# Plain HTTP through one subsystem with a fast backend, with ACME off (TLS on, no
# [data_plane.acme]), and with ACME on for the process and for the subsystem. The
# CA is unreachable on purpose: the runner backs off and nothing is issued, so the
# request path pays exactly what ACME adds to every request (the challenge map's
# emptiness check and one path prefix comparison).
# Needs `oha` and `curl`.
# Usage: scripts/bench-m4.sh <results-file> [rounds]
# Uses ports 13000 (backend), 18000 (proxy HTTP), 18443 (proxy HTTPS) and 19000
# (admin). The admin plane runs with authentication off, on loopback.
set -u
cd "$(dirname "$0")/.."
cargo build --release --examples -p fast-proxy-dataplane 2>&1 | tail -1
cargo build --release 2>&1 | tail -1
BACKEND=target/release/examples/bench_backend
PROXY=target/release/fast-proxy
OUT=$1
ROUNDS=${2:-3}
: > "$OUT"
J='Content-Type: application/json'
A=http://127.0.0.1:19000/api
WORK=$(mktemp -d)

report() { # label result
  local label=$1 result=$2
  rps=$(echo "$result" | awk '/Requests\/sec/ {printf "%d", $2}')
  p50=$(echo "$result" | awk '/50.00% in/ {print $3*1000}')
  p99=$(echo "$result" | awk '/99.00% in/ {print $3*1000}')
  echo "$label | rps=$rps | p50_us=$p50 | p99_us=$p99" | tee -a "$OUT"
}
run() { # label acme(on|off)
  local label=$1 acme=$2
  cat > "$WORK/bench.toml" <<TOML
[data_plane]
state_dir = "$WORK/state-$$-$RANDOM"
[data_plane.tls]
enabled = true
listen = "127.0.0.1:18443"
TOML
  if [[ "$acme" == on ]]; then
    cat >> "$WORK/bench.toml" <<TOML
[data_plane.acme]
enabled = true
directory = "https://127.0.0.1:1/directory"
allowed_domains = ["*.example.com"]
self_check = false
TOML
  fi
  cat >> "$WORK/bench.toml" <<TOML
[[data_plane.listener]]
name = "http"
listen = "127.0.0.1:18000"
[admin]
listen = "127.0.0.1:19000"
auth = "off"
TOML
  $PROXY --config "$WORK/bench.toml" --log-level "info,access=off" --no-sampling >/dev/null 2>&1 &
  PID=$!
  for i in $(seq 1 100); do curl -s $A/health >/dev/null 2>&1 && break; sleep 0.1; done
  curl -s -H "$J" -d '{"name":"bench"}' $A/subsystems >/dev/null
  curl -s -H "$J" -d '{"pattern":"bench.example.com"}' $A/subsystems/1/hosts >/dev/null
  curl -s -H "$J" -d '{"name":"backend","endpoints":[{"addr":"127.0.0.1:13000"}]}' $A/subsystems/1/upstreams >/dev/null
  curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes >/dev/null
  if [[ "$acme" == on ]]; then
    curl -s -X PUT -H "$J" -d '{"name":"bench","https":{"enabled":true,"acmeEnabled":true}}' $A/subsystems/1 >/dev/null
  else
    curl -s -X PUT -H "$J" -d '{"name":"bench","https":{"enabled":true}}' $A/subsystems/1 >/dev/null
  fi
  oha -z 2s -c 64 --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ >/dev/null 2>&1
  report "$label" "$(oha -z 6s -c 64 --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ 2>&1)"
  kill $PID 2>/dev/null; wait $PID 2>/dev/null
}

$BACKEND 127.0.0.1:13000 2 >/dev/null 2>&1 & B1=$!
sleep 1
for round in $(seq 1 "$ROUNDS"); do
  echo "# round $round" | tee -a "$OUT"
  run "acme off" off
  run "acme on " on
done
kill $B1 2>/dev/null
rm -rf "$WORK"
