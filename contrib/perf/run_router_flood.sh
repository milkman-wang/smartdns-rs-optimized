#!/bin/sh
# Flood of unique names with fastest-response + speed check enabled.
# Probes to the fake upstream's answer (192.0.2.1) are routed into a dummy
# interface, so every ping/TCP probe waits for its timeout like an
# unreachable server would. Run inside the dedicated benchmark namespace.
set -eu
[ "$(ip netns identify $$)" = "${PERF_NETNS:-smartdns-flood}" ] || {
    echo "Run with ip netns exec in the dedicated benchmark namespace" >&2; exit 1;
}
cd "${PERF_DIR:-/tmp/smartdns-flood}"
pid=; upstream=
cleanup() {
    for process in "$pid" "$upstream"; do
        [ -z "$process" ] || kill "$process" 2>/dev/null || true
        [ -z "$process" ] || wait "$process" 2>/dev/null || true
    done
    ip route del 192.0.2.0/24 dev qpssink 2>/dev/null || true
    ip link del qpssink 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM

ip link add qpssink type dummy
ip link set qpssink up
ip addr add 198.18.0.1/24 dev qpssink
ip route replace 192.0.2.0/24 dev qpssink

iptables -t mangle -F OUTPUT
iptables -t mangle -A OUTPUT -p udp -d 127.0.0.1 --dport 5301
taskset 8 ./dnsbench server 5301 > flood-upstream.log 2>&1 &
upstream=$!
sleep 1

rss() { awk '/^VmRSS/ {print $2}' "/proc/$1/status"; }
upstream_count() { iptables -t mangle -nvxL OUTPUT | awk '/udp dpt:5301/ {print $1}'; }

: > flood-results.jsonl
for round in 1 2; do
    variants='old new'; [ "$round" != 2 ] || variants='new old'
    for variant in $variants; do
        label=flood-$variant-r$round
        cat > "$label.conf" <<EOF
bind 127.0.0.1:5300
server 127.0.0.1:5301
cache-size 4096
cache-persist no
prefetch-domain no
serve-expired no
response-mode fastest-response
speed-check-mode ping,tcp:443,tcp:80
dualstack-ip-selection no
audit-enable no
log-level error
log-file $PWD/$label.log
num-workers 2
EOF
        taskset 6 ./smartdns-"$variant" run -d "$PWD" -c "$label.conf" > "$label.stdout" 2>&1 &
        pid=$!
        sleep 1
        kill -0 "$pid"
        rss_start=$(rss "$pid")
        iptables -t mangle -Z OUTPUT
        status=0
        taskset 1 ./dnsbench client 5300 20 32 "$pid" 1000000 > "$label.json" || status=$?
        up_during=$(upstream_count)
        rss_end=$(rss "$pid")
        sleep 10
        rss_after=$(rss "$pid")
        up_after=$(upstream_count)
        threads=$(ls "/proc/$pid/task" | wc -l)
        printf '{"round":%s,"variant":"%s","exit_code":%s,"upstream_during":%s,"upstream_after_10s":%s,"rss_start_kb":%s,"rss_end_kb":%s,"rss_after_10s_kb":%s,"threads":%s,"metrics":%s}\n' \
            "$round" "$variant" "$status" "$up_during" "$up_after" "$rss_start" "$rss_end" "$rss_after" "$threads" "$(cat "$label.json")" >> flood-results.jsonl
        echo "$label $(tail -n 1 flood-results.jsonl)"
        kill "$pid"; wait "$pid" 2>/dev/null || true; pid=
    done
done
echo FLOOD COMPLETE
