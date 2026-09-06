#!/bin/sh
# Run inside the benchmark network namespace; see oxidns-20260906.md.
set -eu
dir=${PERF_DIR:-/tmp/smartdns-perf}
engine=$1
binary=$2
workers=$3
scenario=$4
label=$5
domains=256
case "$scenario" in
    cache) ;;
    cold) domains=1000000 ;;
    churn) domains=65536 ;;
    *) exit 2 ;;
esac

case "$engine" in
    smartdns)
        cat > "$dir/compare.conf" <<EOF
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
        taskset e "$binary" run -d "$dir" -c "$dir/compare.conf" > "$dir/compare.log" 2>&1 &
        ;;
    oxidns)
        cat > "$dir/compare.yaml" <<EOF
runtime:
  worker_threads: $workers
log:
  level: error
plugins:
  - tag: cache_main
    type: cache
    args:
      size: 4096
      short_circuit: true
  - tag: forward_main
    type: forward
    args:
      upstreams:
        - addr: "127.0.0.1:5301"
  - tag: seq_main
    type: sequence
    args:
      - exec: \$cache_main
      - exec: \$forward_main
  - tag: udp_server
    type: udp_server
    args:
      entry: seq_main
      listen: "127.0.0.1:5300"
EOF
        taskset e "$binary" start -d "$dir" -c "$dir/compare.yaml" > "$dir/compare.log" 2>&1 &
        ;;
    *) exit 2 ;;
esac
pid=$!
upstream_pid=$(cat "$dir/compare-upstream.pid")
trap 'kill -CONT "$upstream_pid" 2>/dev/null || true; kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$pid"
if ! taskset 1 "$dir/dnsbench" client 5300 1 1 "$pid" > "$dir/compare-warmup.json"; then
    cat "$dir/compare-warmup.json" "$dir/compare.log" >&2
    exit 1
fi
# Cache results must remain successful while the upstream cannot answer.
if [ "$scenario" = cache ]; then kill -STOP "$upstream_pid"; fi
if [ "$scenario" != cache ]; then iptables -t mangle -Z OUTPUT; fi
printf '%s ' "$label"
taskset 1 "$dir/dnsbench" client 5300 5 32 "$pid" "$domains"
if [ "$scenario" != cache ]; then
    printf '%s-upstream ' "$label"
    iptables -t mangle -nvxL OUTPUT | awk '/udp dpt:5301/ {print $1}'
fi
