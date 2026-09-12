#!/bin/sh
# Run INSIDE an isolated network namespace with loopback enabled.
# Directory contains dnsbench and smartdns-{c,previous,headless,webui}.
set -eu
dir=${PERF_DIR:-/tmp/smartdns-perf-20260912}
rounds=${ROUNDS:-3}
duration=${DURATION:-5}
pid=
upstream_pid=
cleanup() {
    for process in "$pid" "$upstream_pid"; do
        [ -z "$process" ] || kill "$process" 2>/dev/null || true
        [ -z "$process" ] || wait "$process" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT TERM
cd "$dir"
: > results.jsonl
taskset 8 "$dir/dnsbench" server 5301 > upstream.log 2>&1 &
upstream_pid=$!
sleep 1
kill -0 "$upstream_pid"
round=1
while [ "$round" -le "$rounds" ]; do
    # Rotate execution order to reduce order/temperature bias.
    case "$((round % 4))" in
        1) variants='c previous headless webui' ;;
        2) variants='previous webui c headless' ;;
        3) variants='webui headless previous c' ;;
        0) variants='headless c webui previous' ;;
    esac
    if [ -n "${VARIANTS:-}" ]; then
        variants=$VARIANTS
        shift_round=1
        while [ "$shift_round" -lt "$round" ]; do
            set -- $variants
            first=$1
            shift
            variants="$* $first"
            shift_round=$((shift_round + 1))
        done
    fi
    for scenario in ${SCENARIOS:-cache cold static latency}; do
        for variant in $variants; do
            label="$scenario-$variant-r$round"
            conf="$dir/$label.conf"
            cat > "$conf" <<EOF
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
log-file $dir/$label.log
EOF
            if [ "$scenario" = static ]; then
                echo 'address /bench/192.0.2.1' >> "$conf"
            fi
            case "$variant" in
                *webui) printf 'webui-enable yes\nwebui-bind 127.0.0.1:6080\n' >> "$conf" ;;
            esac
            if [ "$variant" = c ]; then
                taskset 6 "$dir/smartdns-c" -f -c "$conf" -p - > "$label.stdout" 2>&1 &
            else
                echo 'num-workers 2' >> "$conf"
                taskset 6 "$dir/smartdns-$variant" run -d "$dir" -c "$conf" > "$label.stdout" 2>&1 &
            fi
            pid=$!
            sleep 1
            kill -0 "$pid"
            awk '/VmRSS:|Threads:/ {print}' "/proc/$pid/status" > "$label.idle"
            taskset 1 "$dir/dnsbench" client 5300 1 1 "$pid" 256 > "$label.warmup.json"
            domains=256
            window=32
            rate=0
            seconds=$duration
            [ "$scenario" != cold ] || domains=65536
            if [ "$scenario" = latency ]; then
                window=4
                rate=100
                seconds=10
            fi
            status=0
            taskset 1 "$dir/dnsbench" client 5300 "$seconds" "$window" "$pid" "$domains" "$rate" > "$label.json" || status=$?
            printf '{"round":%s,"scenario":"%s","variant":"%s","exit_code":%s,"metrics":%s}\n' \
                "$round" "$scenario" "$variant" "$status" "$(cat "$label.json")" >> results.jsonl
            echo "$label $(cat "$label.json")"
            kill "$pid"
            wait "$pid" || true
            pid=
            # Delete only this namespace's benchmark upstream flows.
            if [ "$scenario" = cold ]; then
                conntrack -D -p udp --dst 127.0.0.1 --dport 5301 > "$label.conntrack" 2>&1 || true
            fi
        done
    done
    round=$((round + 1))
done
echo COMPLETE > complete
