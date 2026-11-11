#!/bin/bash
# The M5 benchmark behind docs/benchmarks.md: plain HTTP with no maintenance
# (against the M3 numbers), with maintenance on an unrelated subsystem (the idle
# cost of the feature), and with the proxy answering the maintenance page (the
# rate of 503s it can serve). Needs `oha` and `curl`.
# Usage: scripts/bench-m5.sh <results-file>
# Uses ports 13000 (backend), 18000 (proxy HTTP) and 19000 (admin). The admin plane
# runs with authentication off, on loopback, so the script needs no token.
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
WORK=$(mktemp -d)

report() { # label connections result
  rps=$(echo "$3" | awk '/Requests\/sec/ {printf "%d", $2}')
  p50=$(echo "$3" | awk '/50.00% in/ {print $3*1000}')
  p99=$(echo "$3" | awk '/99.00% in/ {print $3*1000}')
  ok=$(echo "$3" | awk '/\[200\]/ {print $2}')
  unavailable=$(echo "$3" | awk '/\[503\]/ {print $2}')
  echo "$1 | c=$2 | rps=$rps | p50_ms=$p50 | p99_ms=$p99 | 200=${ok:-0} | 503=${unavailable:-0}" | tee -a "$OUT"
}
http() { # connections label
  oha -z 2s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ >/dev/null 2>&1
  report "$2" "$1" "$(oha -z 6s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ 2>&1)"
}
start_proxy() {
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
  $PROXY --config "$WORK/bench.toml" --log-level "info,access=off" >/dev/null 2>&1 &
  PROXY_PID=$!
  for i in $(seq 1 100); do curl -s $A/health >/dev/null 2>&1 && break; sleep 0.1; done
}
stop_proxy() { kill $PROXY_PID 2>/dev/null; wait $PROXY_PID 2>/dev/null; }
site() { # name host
  curl -s -H "$J" -d "{\"name\":\"$1\"}" $A/subsystems >/dev/null
}
configure() { # subsystem-id host backend-addr
  curl -s -H "$J" -d "{\"pattern\":\"$2\"}" $A/subsystems/$1/hosts >/dev/null
  curl -s -H "$J" -d "{\"name\":\"backend\",\"endpoints\":[{\"addr\":\"$3\"}]}" $A/subsystems/$1/upstreams >/dev/null
  curl -s -H "$J" -d "{\"prefix\":\"/\",\"upstreamGroupId\":$1}" $A/subsystems/$1/routes >/dev/null
}

$BACKEND 127.0.0.1:13000 2 >/dev/null 2>&1 & B1=$!
sleep 1

start_proxy
site bench; site other
configure 1 bench.example.com 127.0.0.1:13000
configure 2 other.example.com 127.0.0.1:13000

echo "# no maintenance anywhere (the M3 'TLS off' shape)" | tee -a "$OUT"
for c in 1 16 64; do http $c "no maintenance"; done

echo "# maintenance on an unrelated subsystem (idle cost: the bench subsystem is served)" | tee -a "$OUT"
curl -s -X PUT -H "$J" -d '{"message":"other is down"}' $A/subsystems/2/maintenance >/dev/null
for c in 1 16 64; do http $c "maintenance on another subsystem"; done
curl -s -X DELETE $A/subsystems/2/maintenance >/dev/null

echo "# the bench subsystem answers the maintenance page (503, never reaches the backend)" | tee -a "$OUT"
curl -s -X PUT -H "$J" -d '{"message":"deploying"}' $A/subsystems/1/maintenance >/dev/null
for c in 1 16 64; do http $c "maintenance answering"; done
curl -s -X DELETE $A/subsystems/1/maintenance >/dev/null

echo "# the whole proxy in maintenance, with a bypass rule that does not match (the bypass check runs)" | tee -a "$OUT"
curl -s -X PUT -H "$J" -d '{"message":"deploying","bypass":{"cidrs":["10.0.0.0/8"],"header":"X-Bypass"}}' $A/maintenance/global >/dev/null
for c in 1 16 64; do http $c "global maintenance with bypass rules"; done
stop_proxy
kill $B1 2>/dev/null
echo DONE | tee -a "$OUT"
