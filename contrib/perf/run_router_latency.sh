#!/bin/sh
# Run inside an isolated network namespace with the benchmark upstream ready.
set -eu
dir=${PERF_DIR:-/tmp/smartdns-perf}
binary=$1
workers=$2
label=$3

cat > "$dir/latency.conf" <<EOF
bind 127.0.0.1:5300
server 127.0.0.1:5301
cache-size 4096
cache-persist no
prefetch-domain no
serve-expired no
speed-check-mode none
dualstack-ip-selection no
log-level error
num-workers $workers
EOF

# Keep all four cores available; the low-rate client runs on CPU 0.
taskset f "$binary" run -d "$dir" -c "$dir/latency.conf" > "$dir/latency.log" 2>&1 &
pid=$!
upstream_pid=$(cat "$dir/latency-upstream.pid")
trap 'kill -CONT "$upstream_pid" 2>/dev/null || true; kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$pid"
taskset 1 "$dir/dnsbench-paced" client 5300 1 1 "$pid" > "$dir/latency-warmup.json"
kill -STOP "$upstream_pid"
printf '%s ' "$label"
# Fixed offered rate, not a saturation test: 100 QPS, at most 4 outstanding.
taskset 1 "$dir/dnsbench-paced" client 5300 10 4 "$pid" 256 100
