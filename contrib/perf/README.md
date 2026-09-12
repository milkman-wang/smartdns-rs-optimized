# SmartDNS Performance Suite

The [current performance report](../../docs/PERFORMANCE.md) compares C SmartDNS,
upstream Rust and this fork on the Xiaomi BE10000 and Ryzen 7 9800X3D / WSL2,
using main commit 657637e on 2026-09-13.

## Recorded measurements

| Report | Scope |
|---|---|
| [Latest three-way comparison](../../docs/PERFORMANCE.md) | One/two-core throughput, latency and memory on ARM64 and x86_64 |
| [Passwall OOM regression](oom-regression-20260912.md) | Full rule lists, startup memory and IPSet reply timing |
| [ARM PGO and LAN](wire-pgo-20260912.md) | Compiler training, CPU counters and physical LAN measurements |
| [Single-core efficiency](router-efficiency-20260912.md) | Implementation choices and OxiDNS study |
| [Earlier feature validation](performance-goal-20260912.md) | Headless/WebUI behavior and router integration |
| [Earlier optimization measurements](optimization-20260912.md) | Historical development comparisons |
| [Earlier C/Rust measurements](c-rust-20260912.md) | Historical deployed builds; not the current upstream baseline |

Historical reports retain the exact versions tested. Use the current report for
homepage comparisons, rather than treating an old report's “latest” build as
the present main branch.

## CI scenarios

The Python suite below exercises individual runtime paths. It is separate from
the isolated three-way comparison in run_release_comparison.sh.

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
