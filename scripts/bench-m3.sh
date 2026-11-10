#!/bin/bash
# The M3 benchmark behind docs/benchmarks.md: plain HTTP with TLS off (against the
# M2 numbers) and with TLS on, HTTPS keep-alive over HTTP/1.1 and HTTP/2, the
# new-connection (full handshake) rate, a TLS upstream, and the cost of a
# certificate write. Needs `oha`, `curl` and `python3`.
# Usage: scripts/bench-m3.sh <results-file>
# Uses ports 13000 (plain backend), 13100 (TLS backend), 18000 (proxy HTTP),
# 18443 (proxy HTTPS) and 19000 (admin).
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
export FAST_PROXY_MASTER_KEY=$(python3 -c "import base64; print(base64.b64encode(bytes(range(32))).decode())")

report() { # label connections result
  rps=$(echo "$3" | awk '/Requests\/sec/ {printf "%d", $2}')
  p50=$(echo "$3" | awk '/50.00% in/ {print $3*1000}')
  p99=$(echo "$3" | awk '/99.00% in/ {print $3*1000}')
  ok=$(echo "$3" | awk '/\[200\]/ {print $2}')
  echo "$1 | c=$2 | rps=$rps | p50_ms=$p50 | p99_ms=$p99 | ok=$ok" | tee -a "$OUT"
}
http() { # connections label
  oha -z 2s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ >/dev/null 2>&1
  report "$2" "$1" "$(oha -z 6s -c "$1" --no-tui -H "Host: bench.example.com" http://127.0.0.1:18000/ 2>&1)"
}
https() { # connections label extra-oha-args...
  c=$1; label=$2; shift 2
  common=(--no-tui --insecure --connect-to bench.example.com:18443:127.0.0.1:18443)
  oha -z 2s -c "$c" "${common[@]}" "$@" https://bench.example.com:18443/ >/dev/null 2>&1
  report "$label" "$c" "$(oha -z 6s -c "$c" "${common[@]}" "$@" https://bench.example.com:18443/ 2>&1)"
}
start_proxy() { # extra args
  cat > "$WORK/bench.toml" <<TOML
[data_plane]
state_dir = "$WORK/state-$$-$RANDOM"
[[data_plane.listener]]
name = "http"
listen = "127.0.0.1:18000"
[data_plane.tls]
listen = "127.0.0.1:18443"
[admin]
listen = "127.0.0.1:19000"
TOML
  $PROXY --config "$WORK/bench.toml" --log-level "info,access=off" "$@" >/dev/null 2>&1 &
  PROXY_PID=$!
  for i in $(seq 1 100); do curl -s $A/status >/dev/null 2>&1 && break; sleep 0.1; done
}
stop_proxy() { kill $PROXY_PID 2>/dev/null; wait $PROXY_PID 2>/dev/null; }
configure() { # backend-addr [upstream-extra-json]
  curl -s -H "$J" -d '{"name":"bench"}' $A/subsystems >/dev/null
  curl -s -H "$J" -d '{"pattern":"bench.example.com"}' $A/subsystems/1/hosts >/dev/null
  curl -s -H "$J" -d "{\"name\":\"backend\",${2:-}\"endpoints\":[{\"addr\":\"$1\"}]}" $A/subsystems/1/upstreams >/dev/null
  curl -s -H "$J" -d '{"prefix":"/","upstreamGroupId":1}' $A/subsystems/1/routes >/dev/null
}
enable_https() {
  curl -s -X PUT -H "$J" -d '{"name":"bench","https":{"enabled":true}}' $A/subsystems/1 >/dev/null
}

$BACKEND 127.0.0.1:13000 2 >/dev/null 2>&1 & B1=$!
$BACKEND 127.0.0.1:13100 2 --tls "$WORK/ca.pem" >/dev/null 2>&1 & B2=$!
sleep 1

echo "# TLS off (the M2 shape): plain HTTP" | tee -a "$OUT"
start_proxy; configure 127.0.0.1:13000
for c in 1 16 64; do http $c "tls off, http"; done
stop_proxy

echo "# TLS on: plain HTTP is unchanged, HTTPS added" | tee -a "$OUT"
start_proxy --tls; configure 127.0.0.1:13000; enable_https
for c in 1 16 64; do http $c "tls on, http"; done
for c in 1 16 64; do https $c "https h1 keep-alive"; done
for c in 1 16 64; do https $c "https h2" --http2; done
echo "# new connection per request (a full handshake each time)" | tee -a "$OUT"
for c in 1 16 64; do https $c "https h1 new connection" --disable-keepalive; done
stop_proxy

echo "# TLS upstream (HTTP to the proxy, TLS to the backend)" | tee -a "$OUT"
start_proxy --tls
python3 - "$WORK/ca.pem" <<'PY' | curl -s -H "$J" -d @- $A/ca-bundles >/dev/null
import json, sys
print(json.dumps({"name": "bench", "pem": open(sys.argv[1]).read()}))
PY
configure 127.0.0.1:13100 '"tls":{"enabled":true,"serverName":"backend.internal","ca":"bench"},'
for c in 1 16 64; do http $c "tls upstream"; done

echo "# cost of a certificate write (generate + seal + store + compile + swap), ms" | tee -a "$OUT"
python3 - <<'PY' | tee -a "$OUT"
import json, statistics, time, urllib.request
def post(i):
    body = json.dumps({"name": f"w{i}", "names": ["bench.example.com"]}).encode()
    request = urllib.request.Request("http://127.0.0.1:19000/api/certificates/self-signed", body, {"Content-Type": "application/json"})
    started = time.perf_counter(); urllib.request.urlopen(request).read(); return (time.perf_counter() - started) * 1000
times = [post(i) for i in range(60)]
print(f"self-signed write (1..60 certificates stored): median {statistics.median(times):.1f} ms, p95 {sorted(times)[56]:.1f} ms, last {times[-1]:.1f} ms")
PY
stop_proxy
kill $B1 $B2 2>/dev/null
echo DONE | tee -a "$OUT"
