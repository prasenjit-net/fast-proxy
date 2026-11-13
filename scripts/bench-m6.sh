#!/bin/bash
# The M6 benchmark behind docs/benchmarks.md: the cost of request sampling on the
# data plane. Plain HTTP through one subsystem with a fast backend, with sampling
# off (`--no-sampling`), with it on at its defaults, and the same two against a
# backend that is down, where every request is a connectivity failure (the case the
# storm guard exists for), and with body capture switched on for the route (every
# body is teed and kept in memory until the decision, though nothing is stored).
# Needs `oha` and `curl`.
# Usage: scripts/bench-m6.sh <results-file> [rounds]
# Uses ports 13000 (backend), 13001 (a port nothing listens on), 18000 (proxy HTTP)
# and 19000 (admin). The admin plane runs with authentication off, on loopback.
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
  rps=$(echo "$2" | awk '/Requests\/sec/ {printf "%d", $2}')
  p50=$(echo "$2" | awk '/50.00% in/ {print $3*1000}')
  p99=$(echo "$2" | awk '/99.00% in/ {print $3*1000}')
  echo "$1 | rps=$rps | p50_us=$p50 | p99_us=$p99" | tee -a "$OUT"
}
run() { # label backend-addr extra-proxy-args [override-json]
  cat > "$WORK/bench.toml" <<TOML
[data_plane]
state_dir = "$WORK/state-$$-$RANDOM"
[[data_plane.listener]]
name = "http"
listen = "127.0.0.1:18000"
[admin]
listen = "127.0.0.1:19000"
auth = "off"
TOML
  $PROXY --config "$WORK/bench.toml" --log-level "info,access=off" $3 >/dev/null 2>&1 &
  PID=$!
  for i in $(seq 1 100); do curl -s $A/health >/dev/null 2>&1 && break; sleep 0.1; done
  curl -s -H "$J" -d '{"name":"bench"}' $A/subsystems >/dev/null
  curl -s -H "$J" -d '{"pattern":"bench.example.com"}' $A/subsystems/1/hosts >/dev/null
  curl -s -H "$J" -d "{\"name\":\"backend\",\"endpoints\":[{\"addr\":\"$2\"}]}" $A/subsystems/1/upstreams >/dev/null
  curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes >/dev/null
  if [ -n "${4:-}" ]; then
    curl -s -X PUT -H "$J" -d "$4" $A/sampling/overrides >/dev/null
  fi
  oha -z 2s -c 64 --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ >/dev/null 2>&1
  report "$1" "$(oha -z 6s -c 64 --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ 2>&1)"
  kill $PID 2>/dev/null; wait $PID 2>/dev/null
}

$BACKEND 127.0.0.1:13000 2 >/dev/null 2>&1 & B1=$!
sleep 1
for round in $(seq 1 "$ROUNDS"); do
  echo "# round $round" | tee -a "$OUT"
  run "sampling off, fast backend" 127.0.0.1:13000 --no-sampling
  run "sampling on,  fast backend" 127.0.0.1:13000 --sampling
  run "sampling on,  bodies on, fast backend" 127.0.0.1:13000 --sampling '{"subsystemId":1,"captureBodies":true}'
  run "sampling off, backend down" 127.0.0.1:13001 --no-sampling
  run "sampling on,  backend down" 127.0.0.1:13001 --sampling
done
kill $B1 2>/dev/null
rm -rf "$WORK"
