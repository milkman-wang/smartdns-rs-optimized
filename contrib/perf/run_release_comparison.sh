#!/bin/sh
set -eu
# Requires a dedicated namespace: this runner resets its OUTPUT counters.
[ "$(ip netns identify $$)" = "${PERF_NETNS:-smartdns-release-comparison}" ] || {
    echo "Run with ip netns exec in the dedicated benchmark namespace" >&2; exit 1;
}
cd "${PERF_DIR:-/tmp/smartdns-release-comparison}"
pid=; upstream=
cleanup() {
    [ -z "$upstream" ] || kill -CONT "$upstream" 2>/dev/null || true
    for process in "$pid" "$upstream"; do
        [ -z "$process" ] || kill "$process" 2>/dev/null || true
        [ -z "$process" ] || wait "$process" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT TERM
iptables -t mangle -F OUTPUT
iptables -t mangle -A OUTPUT -p udp -d 127.0.0.1 --dport 5301
: > "$RESULTS"
taskset 8 ./dnsbench server 5301 > upstream.log 2>&1 &
upstream=$!
sleep 1
for round in $(seq 1 "${ROUNDS:-3}"); do
    items=$VARIANTS
    shift_round=1
    while [ "$shift_round" -lt "$round" ]; do
        set -- $items; first=$1; shift; items="$* $first"
        shift_round=$((shift_round+1))
    done
    for scenario in ${SCENARIOS:-cold cache latency}; do
        for item in $items; do
            variant=${item%-w?}; workers=${item##*-w}
            mask=6; [ "$workers" != 1 ] || mask=2
            label=$scenario-$item-r$round
            cat > "$label.conf" <<EOF
bind 127.0.0.1:5300
bind-tcp 127.0.0.1:5300
server 127.0.0.1:5301
cache-size 4096
cache-persist no
prefetch-domain no
serve-expired no
speed-check-mode none
dualstack-ip-selection no
audit-enable no
log-level error
log-file $PWD/$label.log
EOF
            case "$variant" in *webui) printf 'webui-enable yes\nwebui-bind 127.0.0.1:6080\n' >> "$label.conf" ;; esac
            if [ "$variant" = c ]; then
                taskset "$mask" ./smartdns-c -f -c "$label.conf" -p - > "$label.stdout" 2>&1 &
            else
                echo "num-workers $workers" >> "$label.conf"
                taskset "$mask" ./smartdns-"$variant" run -d "$PWD" -c "$label.conf" > "$label.stdout" 2>&1 &
            fi
            pid=$!
            sleep 1
            kill -0 "$pid"
            taskset 1 ./dnsbench client 5300 1 1 "$pid" 256 > "$label.warmup.json"
            iptables -t mangle -Z OUTPUT
            domains=256; seconds=5; window=32; rate=0
            case "$scenario" in
                cold) domains=${COLD_DOMAINS:-1000000} ;;
                cache) : ;; # v0.13.1 refreshes on hits; keep the upstream available for every implementation.
                latency) seconds=10; window=4; rate=100 ;;
                loaded-latency) seconds=10; window=4; rate=1000 ;;
            esac
            status=0
            taskset 1 ./dnsbench client 5300 "$seconds" "$window" "$pid" "$domains" "$rate" > "$label.json" || status=$?
            count=$(iptables -t mangle -nvxL OUTPUT | awk '/udp dpt:5301/ {print $1}')
            printf '{"round":%s,"scenario":"%s","variant":"%s","workers":%s,"cpu_mask":"%s","exit_code":%s,"upstream_queries":%s,"metrics":%s}\n' "$round" "$scenario" "$variant" "$workers" "$mask" "$status" "$count" "$(cat "$label.json")" >> "$RESULTS"
            echo "$label $(cat "$label.json") upstream=$count"
            kill -CONT "$upstream"
            kill "$pid"; wait "$pid" || true; pid=
            conntrack -D -p udp --dst 127.0.0.1 --dport 5301 > "$label.conntrack" 2>&1 || true
        done
    done
done
echo COMPLETE
