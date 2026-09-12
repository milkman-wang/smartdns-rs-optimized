# Installation and configuration

[返回首页 / Home](../README.md)

## Installing

*Nightly builds can be found [here](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/nightly.yml).*

- OpenWrt

  The repository includes cross-compilable headless/WebUI packages, procd/UCI/dnsmasq integration, and JavaScript or Lua CBI LuCI applications. See the [OpenWrt integration guide](../contrib/openwrt/README.md) for its support matrix and build steps, and the [performance notes](../contrib/openwrt/PERFORMANCE.md) for a fair Rust/C comparison.

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

For supported directives, examples and differences from C SmartDNS, see the [compatibility guide](../docs/C_FEATURE_COMPATIBILITY.md).

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
