# SmartDNS-rs Optimized

English | [中文](README.md)

A router-focused local DNS server with ad blocking, domain routing and encrypted DNS. Based on [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), this fork improves query performance, resource usage and OpenWrt integration.

[Downloads](https://github.com/milkman-wang/smartdns-rs-optimized/releases) · [OpenWrt installation](contrib/openwrt/README.md) · [Changes](https://github.com/milkman-wang/smartdns-rs-optimized/commits/main/)

## Features

- Ad blocking and routing by domain, client or rule list.
- Multiple upstream resolvers and address reachability checks.
- UDP, TCP, DoT, DoH, DoQ and DoH3.
- OpenWrt LuCI with Chinese translations and an optional standalone WebUI.
- IPv4/IPv6, DHCP hostnames, DNS64 and address-set rules.

## Performance

Tested on 2026-09-13 using the latest main build, including the large-rule-list memory fix. Each table limits the DNS service to **two CPU cores**, with the same local upstream and workload for all three implementations. Values are medians of three runs; **higher means more queries processed per second**.

### Xiaomi BE10000 router

| Version | Uncached queries / second | Cached queries / second |
|---|---:|---:|
| C SmartDNS Release48.4 | 41,939 | 55,138 |
| Upstream Rust v0.13.1 | 10,356 | 8,807 |
| This project (2026-09-13 main) | 49,640 | 115,843 |

### Ryzen 7 9800X3D · WSL2

Ubuntu 24.04 under WSL2 on the same PC, with two virtual CPU cores assigned to each DNS service.

| Version | Uncached queries / second | Cached queries / second |
|---|---:|---:|
| C SmartDNS Release48.4 | 52,231 | 113,606 |
| Upstream Rust v0.13.1 | 24,338 | 28,403 |
| This project (2026-09-13 main) | 98,923 | 400,844 |

These measurements describe local DNS throughput. Public upstream latency, router features and virtualization affect real-world results. See the [detailed report](docs/PERFORMANCE.md) for single-core results, latency, memory, exact binaries and reproduction steps.

## Choose a version

| Version | Intended use |
|---|---|
| Headless | Manage through LuCI or configuration files |
| WebUI | View queries, cache, upstreams and logs directly in a browser |

Both provide the same DNS features. LuCI is a separate OpenWrt interface and works with either version.

Choose a published package from [Releases](https://github.com/milkman-wang/smartdns-rs-optimized/releases), or obtain a recent main build from [GitHub Actions](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/build.yml). Match the architecture and package manager; IPK and APK are different formats. The detailed report identifies the exact artifacts tested above.

## Getting started

- **OpenWrt / QWRT:** follow the [installation guide](contrib/openwrt/README.md), configure upstreams in LuCI and enable the service.
- **Windows / Linux / macOS:** download a matching binary or follow the [source build and configuration guide](docs/USAGE_EN.md).
- The executable and OpenWrt service remain named `smartdns`.

## Documentation

- [Installation, configuration and commands](docs/USAGE_EN.md)
- [Performance methodology and optimization details](docs/PERFORMANCE.md)
- [LuCI interface coverage](contrib/openwrt/INTERFACE_MATRIX.md)
- [C SmartDNS compatibility](docs/C_FEATURE_COMPATIBILITY.md)
- [Build variants](docs/BUILD_VARIANTS.md)

Address sets require kernel support. C fallback, independent HTTP Host and the C plugin ABI remain unsupported; see the compatibility guide for details.

## Credits and license

Thanks to [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), [C SmartDNS](https://github.com/pymumu/smartdns) and [Hickory DNS](https://github.com/hickory-dns/hickory-dns). This fork is maintained by milkman-wang.

Licensed under [GPL-3.0](LICENSE), retaining upstream attribution. Code derived from Hickory DNS retains its Apache-2.0 / MIT notices.
