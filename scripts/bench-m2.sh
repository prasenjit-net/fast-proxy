#!/bin/bash
# The M2 benchmark behind docs/benchmarks.md: a group of one endpoint (to compare
# with the M1 baseline) and groups of 3 and 16 endpoints for every algorithm,
# each through the real release binary against `bench_backend` instances.
# Needs `oha`, `curl` and `python3`. Usage: scripts/bench-m2.sh <results-file>
# Uses ports 13000-13015 (backends), 18000 (proxy) and 19000 (admin).
set -u
cd "$(dirname "$0")/.."
cargo build --release --examples -p fast-proxy-dataplane 2>&1 | tail -1
cargo build --release 2>&1 | tail -1
BACKEND=target/release/examples/bench_backend
PROXY=target/release/fast-proxy
OUT=$1
: > "$OUT"
J='Content-Type: application/json'
A=http://127.0.0.1:19000/api
run_oha() { # connections label
  oha -z 2s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ >/dev/null 2>&1
  res=$(oha -z 6s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ 2>&1)
  rps=$(echo "$res" | awk '/Requests\/sec/ {printf "%d", $2}')
  p50=$(echo "$res" | awk '/50.00% in/ {print $3*1000}')
  p99=$(echo "$res" | awk '/99.00% in/ {print $3*1000}')
  ok=$(echo "$res" | awk '/\[200\]/ {print $2}')
  echo "$2 | c=$1 | rps=$rps | p50_ms=$p50 | p99_ms=$p99 | ok=$ok" | tee -a "$OUT"
}
start_proxy() {
  dir=$(mktemp -d)
  cat > "$dir/bench.toml" <<TOML
[data_plane]
state_dir = "$dir/state"
[[data_plane.listener]]
name = "http"
listen = "127.0.0.1:18000"
[admin]
listen = "127.0.0.1:19000"
TOML
  $PROXY --config "$dir/bench.toml" --log-level "info,access=off" >/dev/null 2>&1 &
  PROXY_PID=$!
  for i in $(seq 1 100); do curl -s $A/status >/dev/null 2>&1 && break; sleep 0.1; done
}
stop_proxy() { kill $PROXY_PID 2>/dev/null; wait $PROXY_PID 2>/dev/null; }
configure() { # endpoints-json algorithm
  curl -s -H "$J" -d '{"name":"bench"}' $A/subsystems >/dev/null
  curl -s -H "$J" -d '{"pattern":"bench.example.com"}' $A/subsystems/1/hosts >/dev/null
  curl -s -H "$J" -d "{\"name\":\"backend\",\"lb\":{\"algorithm\":\"$2\"},\"endpoints\":$1}" $A/subsystems/1/upstreams >/dev/null
  curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes >/dev/null
}
BASE=13000
pids=()
for i in $(seq 0 15); do $BACKEND 127.0.0.1:$((BASE+i)) 2 >/dev/null 2>&1 & pids+=($!); done
sleep 1
endpoints() { python3 -c "import json,sys; print(json.dumps([{'addr':'127.0.0.1:%d'%(13000+i)} for i in range($1)]))"; }

echo "# direct (reference, same run)" | tee -a "$OUT"
for c in 1 16 64; do
  res=$(oha -z 6s -c $c --no-tui http://127.0.0.1:13000/ 2>&1)
  echo "direct | c=$c | rps=$(echo "$res" | awk '/Requests\/sec/ {printf "%d", $2}') | p50_ms=$(echo "$res" | awk '/50.00% in/ {print $3*1000}') | p99_ms=$(echo "$res" | awk '/99.00% in/ {print $3*1000}')" | tee -a "$OUT"
done

echo "# one endpoint (the M1 shape), round robin" | tee -a "$OUT"
start_proxy; configure "$(endpoints 1)" round_robin
for c in 1 16 64; do run_oha $c "1 endpoint"; done
stop_proxy

for n in 3 16; do
  for alg in round_robin weighted_round_robin least_connections p2c_ewma consistent_hash; do
    echo "# $n endpoints, $alg" | tee -a "$OUT"
    start_proxy; configure "$(endpoints $n)" $alg
    run_oha 64 "$n endpoints $alg"
    stop_proxy
  done
done
kill "${pids[@]}" 2>/dev/null
echo DONE | tee -a "$OUT"
