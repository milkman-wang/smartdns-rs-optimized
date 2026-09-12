#!/bin/sh
set -eu
[ "$(ip netns identify $$)" = smartdns-oom-perf-20260912 ]
cd /tmp/smartdns-oom-perf-20260912
pid=; upstream=; upstream2=
cleanup() {
    for p in "$pid" "$upstream" "$upstream2"; do
        [ -z "$p" ] || kill "$p" 2>/dev/null || true
        [ -z "$p" ] || wait "$p" 2>/dev/null || true
    done
}
trap cleanup EXIT
trap 'exit 130' INT TERM
iptables -t mangle -A OUTPUT -p udp -d 127.0.0.1 --dport 5301
ipset create bench_cn hash:ip
ipset create bench_gfw hash:ip
taskset 8 ./dnsbench server 5301 > upstream.log 2>&1 & upstream=$!
taskset 8 ./dnsbench server 5302 > upstream2.log 2>&1 & upstream2=$!
echo '{"phase":"initial"}' > progress.json
: > results.jsonl
for round in 1 2 3; do
    variants='before after'; [ "$round" != 2 ] || variants='after before'
    for profile in plain lists ipset; do
        for workers in 1 2; do
            mask=2; [ "$workers" = 1 ] || mask=6
            for scenario in cold cache latency; do
                for variant in $variants; do
                    available=$(awk '/MemAvailable:/ {print $2}' /proc/meminfo)
                    [ "$available" -gt 200000 ] || { echo "STOP: insufficient free memory $available KiB"; exit 1; }
                    label="$profile-$scenario-$variant-w$workers-r$round"
                    cat > "$label.conf" <<EOF
bind 127.0.0.1:5300
bind-tcp 127.0.0.1:5300
server 127.0.0.1:5301 -group China
server 127.0.0.1:5302 -group overseas -exclude-default-group
num-workers $workers
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
                    if [ "$profile" != plain ]; then
                        sets_cn=; sets_gfw=
                        if [ "$profile" = ipset ]; then
                            sets_cn='-ipset #4:bench_cn'; sets_gfw='-ipset #4:bench_gfw'
                        fi
                        cat >> "$label.conf" <<EOF
domain-set -name cn -file $PWD/chnlist
domain-set -name gfw -file $PWD/gfwlist
domain-rules /./ -nameserver China
domain-rules /domain-set:gfw/ -nameserver overseas $sets_gfw -speed-check-mode none -d no -no-serve-expired -address -6
domain-rules /domain-set:cn/ -nameserver China $sets_cn -d no -c none -no-serve-expired -r fastest-response -rr-ttl-min 1 -rr-ttl-max 300 -address -6
EOF
                    fi
                    ipset flush bench_cn; ipset flush bench_gfw
                    sh -c 'echo 1000 > /proc/self/oom_score_adj; exec taskset "$1" "$2" run -d "$3" -c "$4"' sh "$mask" "./smartdns-$variant" "$PWD" "$label.conf" > "$label.stdout" 2>&1 & pid=$!
                    ready=0
                    until grep -q ':14B4 ' /proc/net/udp; do
                        kill -0 "$pid"
                        ready=$((ready+1)); [ "$ready" -lt 15 ] || { echo "STARTUP FAILED $label"; exit 1; }
                        sleep 1
                    done
                    sleep 1
                    startup_rss=$(awk '/VmRSS:/ {print $2}' /proc/$pid/status)
                    startup_hwm=$(awk '/VmHWM:/ {print $2}' /proc/$pid/status)
                    taskset 1 ./dnsbench-jd client 5300 1 1 "$pid" 256 > "$label.warmup.json"
                    iptables -t mangle -Z OUTPUT
                    domains=256; seconds=5; window=32; rate=0
                    case "$scenario" in
                        cold) domains=1000000 ;;
                        latency) seconds=10; window=4; rate=1000 ;;
                    esac
                    status=0
                    taskset 1 ./dnsbench-jd client 5300 "$seconds" "$window" "$pid" "$domains" "$rate" > "$label.json" || status=$?
                    count=$(iptables -t mangle -nvxL OUTPUT | awk '/udp dpt:5301/ {print $1}')
                    hwm=$(awk '/VmHWM:/ {print $2}' /proc/$pid/status)
                    printf '{"round":%s,"profile":"%s","scenario":"%s","variant":"%s","workers":%s,"exit_code":%s,"upstream_queries":%s,"startup_rss_kb":%s,"startup_hwm_kb":%s,"hwm_kb":%s,"metrics":%s}\n' "$round" "$profile" "$scenario" "$variant" "$workers" "$status" "$count" "$startup_rss" "$startup_hwm" "$hwm" "$(cat "$label.json")" >> results.jsonl
                    echo "$label $(cat "$label.json")"
                    [ "$profile" != ipset ] || ipset test bench_cn 192.0.2.1
                    kill "$pid"; wait "$pid" || true; pid=
                    conntrack -D -p udp --dst 127.0.0.1 --dport 5301 >/dev/null 2>&1 || true
                    [ "$status" -eq 0 ] || exit "$status"
                done
            done
        done
    done
done
echo COMPLETE
