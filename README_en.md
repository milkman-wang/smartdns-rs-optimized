# SmartDNS-rs Optimized

![Test](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/test.yml/badge.svg?branch=main)
[![GitHub release (latest by date including pre-releases)](https://img.shields.io/github/v/release/milkman-wang/smartdns-rs-optimized?display_name=tag&include_prereleases)](https://github.com/milkman-wang/smartdns-rs-optimized/releases)
![OS](https://img.shields.io/badge/os-Windows%20%7C%20MacOS%20%7C%20Linux-blue)

[Docs](docs/C_FEATURE_COMPATIBILITY.md) · [C SmartDNS upstream docs](https://pymumu.github.io/smartdns/en/)

English | [中文](README.md)

SmartDNS-rs is a local DNS server that queries multiple upstream resolvers and can select the fastest reachable address. This is **milkman-wang's maintained fork** of [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), itself inspired by [C SmartDNS](https://github.com/pymumu/smartdns).

This fork focuses on router CPU efficiency, cold-query and cache performance, native Rust DNS features, and OpenWrt integration. Changes maintained here through **2026-09-12** include UDP connection reuse, fewer response allocations, TTL/encoded-response reuse within the cache budget, a single-worker runtime, ARM64 PGO builds, optional Rust WebUI, and JS/Lua LuCI with Chinese translations. Original authorship and license notices are retained.

The repository is named `smartdns-rs-optimized`; the executable remains `smartdns` and OpenWrt package/service names remain unchanged. Development is consolidated on **`main`**. Headless and WebUI are build variants of the same source, with separate release tags; they do not require separate maintenance branches.

## Downloads and documentation

- **ARM64 PGO 0.13.1-24:** [Headless](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-v0.13.1-r24-pgo) / [WebUI](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-webui-v0.13.1-r24-pgo). These releases provide ARM64 musl IPK and binary archives, plus JS/Lua LuCI packages; they do not contain APK or other architectures.
- [Release notes](docs/releases/openwrt-0.13.1-r24-pgo.md) · [Build variants](docs/BUILD_VARIANTS.md) · [OpenWrt installation](contrib/openwrt/README.md) · [LuCI support matrix](contrib/openwrt/INTERFACE_MATRIX.md).
- [C feature compatibility](docs/C_FEATURE_COMPATIBILITY.md): native ipset/nftset, TCP SYN probing, SPKI, DDR, certificate generation and local records. Kernel features are required for sets. C fallback, independent HTTP Host and the C plugin ABI remain unsupported.

## Performance comparison

Measured on a **Xiaomi BE10000 / Cortex-A73, QWRT 25.12.2, Linux 5.4.213**. Values below are three-run medians from the same 96-test batch. “Before” is this fork's preceding TTL-reuse build; “PGO” adds encoded-response reuse and profile-guided compilation. **This is not a comparison against unmodified upstream SmartDNS-rs.**

| Variant / DNS workers | Cold QPS before → PGO | Change | Cached QPS before → PGO | Change |
|---|---:|---:|---:|---:|
| Headless / 1 | 24,784 → 30,783 | +24.2% | 57,290 → 77,412 | +35.1% |
| Headless / 2 | 43,766 → 53,301 | +21.8% | 105,274 → 115,564 | +9.8% |
| WebUI / 1 | 23,878 → 28,981 | +21.4% | 51,780 → 69,718 | +34.6% |
| WebUI / 2 | 42,242 → 50,324 | +19.1% | 95,517 → 114,109 | +19.5% |

The benchmark uses a local fixed upstream, 4,096 cache entries, 256 hot names, one million non-repeating cold names, and 32 outstanding requests. Speed probing, dual-stack selection, prefetch, stale replies and auditing are disabled; WebUI history remains enabled for that variant. Each cold run verifies upstream query counts; cached throughput is measured with the upstream paused. All 96 tests had zero errors/timeouts. Ordinary `cargo build --release` does **not** include PGO automatically.

A separate two-generator cached-capacity comparison reached **132,333 QPS**, versus 102,010 before (+29.7%), with two server workers and 32 total outstanding requests. Wired LAN P99 improved approximately **4–11%** with normal scheduling. In a 20-second WebUI cold-query run, maximum sampled RSS fell from **16.12 to 12.64 MiB**; this small benchmark configuration is not the memory footprint of a full router deployment or an RSS limit.

See the [full PGO report and reproduction steps](contrib/perf/wire-pgo-20260912.md) and [raw results](contrib/perf/wire-pgo-20260912.json).

### C SmartDNS / OxiDNS reference

An earlier, separate batch compared a **pre-PGO build of this fork** against C SmartDNS Release48.4 and OxiDNS v1.5.2 full ARM64 musl. CPU counts below are affinity limits; each implementation retains its own thread model.

| Implementation / CPU cores | Cold QPS | Cached QPS | Cold end-of-run RSS, MiB |
|---|---:|---:|---:|
| This fork, pre-PGO / 1 | 24,656 | 52,908 | 14.35 |
| This fork, pre-PGO / 2 | 43,826 | 97,648 | 15.07 |
| C SmartDNS / 1 | 17,857 | 54,776 | 6.89 |
| C SmartDNS / 2 | 40,538 | 54,580 | 6.85 |
| OxiDNS / 1 | 17,013 | 44,119 | 120.94 |
| OxiDNS / 2 | 29,777 | 80,775 | 211.41 |

These measurements cover the specified versions and simple configurations. Cache eviction semantics differ, especially for OxiDNS, so RSS is not a universal comparison. Do not combine this batch with the PGO table to calculate a current-release speedup over C or OxiDNS. We did not benchmark OxiDNS against mosdns. [Methods, limitations and raw data](contrib/perf/router-efficiency-20260912.md#c--oxidns-参考批次).

Public upstream RTT, TLS/QUIC setup, probing policy, larger rule sets and other SoCs can change the result. The measured gains come from implementation and compilation changes, not from a general claim that Rust is faster than C or Go.

## Features

See [C SmartDNS feature compatibility](docs/C_FEATURE_COMPATIBILITY.md) for
configuration examples, plugin support and platform requirements.

- **Multiple upstream DNS servers**

  Supports configuring multiple upstream DNS servers and query at the same  time.the query will not be affected, Even if there is a DNS server  exception.

- **Return the fastest IP address**

  Supports finding the fastest access IP address from the IP address list  of the domain name and returning it to the client to avoid DNS pollution and improve network access speed.

- **Support for multiple query protocols**

  Supports UDP, TCP, DoT, DoQ, DoH, DoH3 queries and  service, and non-53 port queries, effectively avoiding DNS pollution and protect privacy, and support query DNS over socks5, http proxy.

- **Domain IP address specification**

  Supports configuring IP address of specific domain to achieve the effect of advertising filtering, and avoid malicious websites.

- **DNS domain forwarding**

  Supports DNS forwarding, ipset and nftables. Support setting the domain result to ipset and nftset set when speed check fails.

- **Windows / MacOS / Linux multi-platform support**

  Supports installing as a service and running it at startup.

- **Support IPV4, IPV6 dual stack**

  Supports IPV4, IPV6 network, support query A, AAAA record, dual-stack IP selection, and filter IPV6 AAAA record.

- **DNS64**

  Supports DNS64 translation.

- **High performance, low resource consumption**

  Tokio-based asynchronous I/O with a current-thread runtime for one DNS worker, cache budgets, prefetch and reusable responses. Performance and memory depend on configuration and workload; see the measurements above.

Platform and feature coverage are documented in the [compatibility guide](docs/C_FEATURE_COMPATIBILITY.md). Router performance results apply to the tested ARM64 environment.

## Installing

*Nightly builds can be found [here](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/nightly.yml).*

- OpenWrt

  The repository includes cross-compilable headless/WebUI packages, procd/UCI/dnsmasq integration, and JavaScript or Lua CBI LuCI applications. See the [OpenWrt integration guide](contrib/openwrt/README.md) for its support matrix and build steps, and the [performance notes](contrib/openwrt/PERFORMANCE.md) for a fair Rust/C comparison.

- MacOS

  The Homebrew formula installs the upstream project, not this fork's optimizations. To build this fork on macOS, use the source build instructions below. The upstream package remains available with:

  ```shell
  brew update
  brew install smartdns
  ```

  Note: Listening on port 53 requires root permission, so `sudo` is required.

  The command `sudo smartdns service start` for `brew` installed `smartdns` is the same as `sudo brew services start smartdns`.

  If you don't have `brew` installed, just download the compiled program compression package and install it as below.

- Windows / Linux

  Download a matching platform asset from [this fork's releases](https://github.com/milkman-wang/smartdns-rs-optimized/releases), or build from source if none is available. The ARM64 PGO release cannot run on Windows or x86. After extracting a matching binary:

  1. Get help

     ```shell
     ./smartdns --help
     ```

  2. Run as foreground, easy to check the running status

     ```shell
     ./smartdns run -c ./smartdns.conf -v
     ```

     - `-v` is enabled to print debug logs.

  3. Run as background service, run automatically at startup

     Get help of service management commands.

     ```shell
     ./smartdns service --help
     ```

     *Note: Installed as a system service, administrator / root permissions are required.*

     *Service management is compatible with all systems, call [sc](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-server-2012-r2-and-2012/cc754599(v=ws.11)) on Windows; call `launchctl` or `brew` on MacOS; call `Systemd` or `OpenRc` on Linux.*

## Configuration

The following is the simplest example configuration

```conf
# Listen on local port 53
bind 127.0.0.1:53  

# Configure bootstrap-dns, if not configured, call the system_conf, 
# it is recommended to configure, so that it will be encrypted.
server https://1.1.1.1/dns-query  -bootstrap-dns -exclude-default-group
server https://8.8.8.8/dns-query  -bootstrap-dns -exclude-default-group

# Configure default upstream server
server https://cloudflare-dns.com/dns-query
server https://dns.quad9.net/dns-query
server https://dns.google/dns-query

# Configure the Office(Home) upstream server
server 192.168.1.1 -exclude-default-group -group office

# Domain names ending with ofc are forwarded to the office group for resolution
nameserver /ofc/office

# Set static IP for domain name
address /test.example.com/1.2.3.5

# Block Domains (Ad Blocking)
address /ads.example.com/#

# The following features are not yet supported in the [C SmartDNS](https://github.com/pymumu/smartdns) and are only applicable to SmartDNS-rs.
# Configure DoH3
server-h3 1.1.1.1

# Configure DoQ
server-quic unfiltered.adguard-dns.com
```

For supported directives, examples and differences from C SmartDNS, see the [compatibility guide](docs/C_FEATURE_COMPATIBILITY.md).

## Built-in diagnostics via `dig`

SmartDNS-rs supports built-in `CHAOS TXT` queries for server/client diagnostics.

```shell
# most common: full identity info (server + client, multi TXT records)
dig @127.0.0.1 CH TXT whoami +short

# server identity info only (multi TXT records)
dig @127.0.0.1 CH TXT smartdns +short

# server name
dig @127.0.0.1 CH TXT server-name +short

# server version
dig @127.0.0.1 CH TXT version +short

# client source IP seen by smartdns-rs
dig @127.0.0.1 CH TXT client_ip +short
dig @127.0.0.1 CH TXT client-ip +short

# client MAC from ARP table (LAN, ARP available)
dig @127.0.0.1 CH TXT client_mac +short
dig @127.0.0.1 CH TXT client-mac +short

# JSON output with suffix style
dig @127.0.0.1 CH TXT whoami.json +short
dig @127.0.0.1 CH TXT smartdns.json +short

# Compatibility examples
dig @127.0.0.1 CH TXT hostname.bind +short
dig @127.0.0.1 CH TXT version.bind +short
dig @127.0.0.1 CH TXT id.server +short
```

## Building

Assuming you have installed [Rust](https://www.rust-lang.org/learn/get-started), then you can open the terminal and execute these commands:

```shell
git clone https://github.com/milkman-wang/smartdns-rs-optimized.git
cd smartdns-rs-optimized

# install https://github.com/casey/just
cargo install just

# build
just build --release

# print help
./target/release/smartdns --help

# run
sudo ./target/release/smartdns run -c ./etc/smartdns/smartdns.conf
```

For cross-compilation, it is recommended to use [cross](https://github.com/cross-rs/cross) (requires Docker).

## Acknowledgments !!!

This software wouldn't have been possible without:

- [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs), the upstream Rust implementation
- [Hickory DNS](https://github.com/hickory-dns/hickory-dns)
- [SmartDNS](https://github.com/pymumu/smartdns)

## License

This software contains codes from [https://github.com/hickory-dns/hickory-dns](https://github.com/hickory-dns/hickory-dns), which is licensed under either of

- Apache License, Version 2.0, (LICENSE-APACHE or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))
- MIT license (LICENSE-MIT or [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))

And other codes is licensed under

- GPL-3.0 license (LICENSE-GPL-3.0 or [https://opensource.org/licenses/GPL-3.0](https://opensource.org/licenses/GPL-3.0))

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the GPL-3.0 license, shall be licensed as above, without any additional terms or conditions.
