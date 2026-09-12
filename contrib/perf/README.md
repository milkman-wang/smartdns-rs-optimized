# SmartDNS Performance Suite

This directory contains a scenario-based performance suite that targets specific runtime paths.

The [performance goal report](performance-goal-20260912.md) compares the latest
headless and WebUI builds against C Release48.4 and their previous optimized
builds, with repeated throughput, CPU cost, latency, and router feature checks.

The [2026-09-12 optimization report](optimization-20260912.md) compares the
finished optimization with both pre-optimization variants, the previous Rust
version, and C, including CPU cost and live WebUI validation.

The [2026-09-12 C and Rust comparison](c-rust-20260912.md) measures C Release48.4,
the previous deployed Rust version, and the current headless/WebUI variants on
the same router. It includes raw results and a reproducible isolated runner.

## Scenarios and covered functionality

The [encoded response, PGO, and physical LAN report](wire-pgo-20260912.md)
compares the next optimization with the previous final build, including CPU
target screening, ARM PMU counters, load-generator capacity, and process affinity.
Its final packages use PGO; ordinary release builds receive the source changes
without automatically applying the trained profile.

The [single-core efficiency and OxiDNS implementation study](router-efficiency-20260912.md)
records the next round of measured changes, including 96 final trials, bounded
TTL response reuse, cold-query memory samples, and latency tradeoffs. Its
[isolated runner](run_router_efficiency.sh) verifies upstream query counts.

| Scenario | Covered functionality |
|---|---|
| `static_address_rule` | `AddressMiddleware` static response path, local UDP server path |
| `hosts_file_lookup` | `DnsHostsMiddleware` hosts-file lookup path, hosts file mtime/signature cache |
| `dnsmasq_lease_lookup` | `DnsmasqMiddleware` path, `LanClientStore` lease-file mtime cache |
| `dns_cache_hit_path` | `DnsCacheMiddleware` insert/get + cache-hit path + direct `DnsHandle` dispatch |
| `prefetch_scheduler_active` | cache prefetch scheduling/index path with large cached domain set |

## Local run

```bash
just build --release --features disable_icmp_ping

python3 contrib/perf/run_perf_suite.py \
  --binary target/release/smartdns \
  --duration-sec 8 \
  --concurrency 64 \
  --timeout-ms 500 \
  --hosts-records 12000 \
  --lease-records 2000 \
  --prefill-domains 3000 \
  --repeats 1 \
  --output-json artifacts/perf/results.json \
  --output-md artifacts/perf/summary.md
```

The script prints the Markdown summary to stdout and writes:

- JSON metrics: `artifacts/perf/results.json`
- Markdown summary: `artifacts/perf/summary.md`

Useful knobs:

- `--hosts-records`: scales hosts-file size for `DnsHostsMiddleware` cache-path sensitivity
- `--lease-records`: scales dnsmasq lease-file size for cache-path sensitivity
- `--prefill-domains`: scales cached-domain count for prefetch scheduler pressure
- `--repeats`: repeats each scenario and uses median values to reduce run-to-run noise

## Compare two branches (or binaries)

```bash
python3 contrib/perf/compare_perf.py \
  --base artifacts/perf/main.json \
  --target artifacts/perf/current.json \
  --base-name main \
  --target-name current \
  --output-md artifacts/perf/compare.md
```

## CI output

The GitHub workflow `.github/workflows/perf.yml` runs this suite and publishes:

1. Job Summary table (QPS, success rate, p50/p95/p99, avg latency)
2. Uploaded artifact containing both JSON and Markdown outputs
