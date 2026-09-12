# SmartDNS-rs Optimized

English | [中文](README.md)

A router-focused local DNS server with ad blocking, domain routing and encrypted DNS. Based on [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), this fork improves query performance, resource usage and OpenWrt integration.

[Downloads](https://github.com/milkman-wang/smartdns-rs-optimized/releases) · [OpenWrt installation](contrib/openwrt/README.md) · [Release notes](docs/releases/openwrt-0.13.1-r24-pgo.md)

## Features

- Ad blocking and routing by domain, client or rule list.
- Multiple upstream resolvers and address reachability checks.
- UDP, TCP, DoT, DoH, DoQ and DoH3.
- OpenWrt LuCI with Chinese translations and an optional standalone WebUI.
- IPv4/IPv6, DHCP hostnames, DNS64 and address-set rules.

## Performance

### Xiaomi BE10000 router

All three implementations use the same local upstream and workload, limited to two CPU cores. Values are medians of three runs; **higher means more queries processed per second**.

| Version | Uncached queries / second | Cached queries / second |
|---|---:|---:|
| C SmartDNS Release48.4 | 41,232 | 55,371 |
| Upstream Rust v0.13.1 | 10,508 | 9,228 |
| This project 0.13.1-24 | 53,890 | 116,835 |

Tested on 2026-09-12. This project processes more queries per second in this workload; C SmartDNS has lower tail latency at low load. These results are not public DNS latency or page load times. Single-core results, latency, memory, exact versions and reproduction steps are in the [detailed report](docs/PERFORMANCE.md).

## Choose a version

| Version | Intended use |
|---|---|
| Headless | Manage through LuCI or configuration files |
| WebUI | View queries, cache, upstreams and logs directly in a browser |

Both provide the same DNS features. LuCI is a separate OpenWrt interface and works with either version.

The current ARM64 router releases are available as [headless](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-v0.13.1-r24-pgo) and [WebUI](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-webui-v0.13.1-r24-pgo). Choose assets for your architecture and package manager. These releases provide IPK, not APK.

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

Thanks to [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), [C SmartDNS](https://github.com/pymumu/smartdns) and [Hickory DNS](https://github.com/hickory-dns/hickory-dns). This fork is maintained by milkman-wang; recent modifications are documented in the 2026-09-12 release notes.

Licensed under [GPL-3.0](LICENSE), retaining upstream attribution. Code derived from Hickory DNS retains its Apache-2.0 / MIT notices.
