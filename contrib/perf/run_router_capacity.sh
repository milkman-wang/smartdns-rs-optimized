#!/bin/sh
set -eu
[ "$(ip netns identify $$)" = "${PERF_NETNS:-smartdns-wire-20260912}" ] || {
    echo "Run inside the dedicated benchmark namespace" >&2; exit 1;
}
cd "${PERF_DIR:-/tmp/smartdns-wire-20260912}"
pid=; upstream=; first=; second=
cleanup() {
    [ -z "$upstream" ] || kill -CONT "$upstream" 2>/dev/null || true
    for process in "$first" "$second" "$pid" "$upstream"; do
        [ -z "$process" ] || kill "$process" 2>/dev/null || true
        [ -z "$process" ] || wait "$process" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT TERM
iptables -t mangle -Z OUTPUT
taskset 8 ./dnsbench server 5301 > capacity-upstream.log 2>&1 & upstream=$!
: > capacity-results.jsonl
for round in 1 2 3; do
    variants='before final'; [ "$round" != 2 ] || variants='final before'
    modes='1 2'; [ "$round" != 2 ] || modes='2 1'
    for clients in $modes; do
        for variant in $variants; do
            label=capacity-$variant-c$clients-r$round
            cp cache-$variant-w2-r1.conf "$label.conf"
            taskset 6 ./smartdns-$variant run -d "$PWD" -c "$label.conf" > "$label.stdout" 2>&1 & pid=$!
            sleep 1
            taskset 1 ./dnsbench client 5300 1 1 "$pid" 256 > "$label.warmup.json"
            iptables -t mangle -Z OUTPUT
            kill -STOP "$upstream"
            if [ "$clients" = 1 ]; then
                taskset 1 ./dnsbench client 5300 5 32 "$pid" 256 > "$label-a.json"
                metrics="$(cat "$label-a.json")"
            else
                taskset 1 ./dnsbench client 5300 5 16 "$pid" 256 > "$label-a.json" & first=$!
                taskset 8 ./dnsbench client 5300 5 16 "$pid" 256 > "$label-b.json" & second=$!
                wait "$first"; first=
                wait "$second"; second=
                metrics="$(cat "$label-a.json"),$(cat "$label-b.json")"
            fi
            count=$(iptables -t mangle -nvxL OUTPUT | awk '/udp dpt:5301/ {print $1}')
            printf '{"round":%s,"variant":"%s","clients":%s,"upstream_queries":%s,"metrics":[%s]}\n' "$round" "$variant" "$clients" "$count" "$metrics" >> capacity-results.jsonl
            echo "$label completed"
            kill -CONT "$upstream"
            kill "$pid"; wait "$pid" || true; pid=
        done
    done
done
echo CAPACITY_COMPLETE
