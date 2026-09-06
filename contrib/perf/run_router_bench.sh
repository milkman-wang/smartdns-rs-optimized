#!/bin/sh
# Requires router_dnsbench's server mode listening on 127.0.0.1:5301.
set -eu

dir=${PERF_DIR:-/tmp/smartdns-perf}
binary=$1
workers=$2
scenario=$3
label=$4
domains=256

case "$scenario" in
	cache|static) ;;
	cold) domains=65536 ;;
	*) echo "Unknown scenario: $scenario" >&2; exit 2 ;;
esac

cat > "$dir/test.conf" <<EOF
bind 127.0.0.1:5300
bind-tcp 127.0.0.1:5300
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
if [ "$scenario" = static ]; then
	echo 'address /bench/192.0.2.1' >> "$dir/test.conf"
fi

# A four-core router: reserve CPU 0 for the load generator and upstream.
taskset "${SERVER_CPU_MASK:-e}" "$binary" run -d "$dir" -c "$dir/test.conf" \
	> "$dir/server.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT INT TERM
sleep 1
kill -0 "$pid"
taskset 1 "$dir/dnsbench" client 5300 1 1 "$pid" > "$dir/warmup.json"
printf '%s ' "$label"
taskset 1 "$dir/dnsbench" client 5300 5 32 "$pid" "$domains"
