# C SmartDNS feature compatibility

This document describes the additions based on C SmartDNS **Release48.4**. The
configuration examples below apply to SmartDNS-rs; platform requirements are
listed where they affect operation.

## Cache and request handling

| Configuration / behavior | Implementation |
| --- | --- |
| NXDOMAIN and NODATA caching | Stores the complete response, including SOA and CNAME records. Lifetime is bounded by SOA TTL, SOA MINIMUM and any CNAME TTL. Negative entries are never served stale or proactively refreshed. Responses without a usable SOA lifetime bypass negative caching. |
| `cache-size -1` | Automatic entry capacity based on 1% of physical memory, estimated at 1 KiB per entry, bounded to 512–1,048,576 entries. `0` disables caching. |
| `cache-mem-size 16MiB` | Evicts LRU entries when their accounted DNS payload and record/cache storage exceed the budget. This is cache accounting, not a process RSS limit. `0` uses only the entry limit. |
| `serve-expired-prefetch-time 300` | With stale serving and prefetch enabled, schedules positive-record refresh after expiry plus this interval. `0` selects half the stale lifetime, capped at eight hours; an unlimited stale lifetime uses eight hours. Failed refreshes release the in-flight marker and retry after the interval. |
| Cache persistence | Preserves complete response sections, probe results and absolute expiry times across restart. Existing cache files remain readable. |
| `max-query-limit 128` | Limits foreground queries; excess queries receive REFUSED. Background refreshes retain their separate concurrency limit. `0` disables the foreground limit. |
| Listener `-no-serve-expired` | Applies alongside global and domain stale-response policy. |
| Upstream `-check-edns` | Rejects upstream responses that omit EDNS. |

## Client rules, local records and network sets

```conf
client-rules 01:23:45:67:89:ab -group devices
group-begin devices
address /printer.home/192.168.1.20
group-end

odhcpd-lease-file /tmp/hosts/odhcpd
domain home
txt-record /service.home/"first TXT chunk" "second TXT chunk"

speed-check-mode tcp-syn:443

ipset /example.com/#4:route4,#6:route6
ipset-timeout yes
ipset-no-speed #4:slow4,#6:slow6
nftset /example.net/#4:inet#fw4#route4,#6:inet#fw4#route6
nftset-timeout yes
nftset-no-speed #4:inet#fw4#slow4,#6:inet#fw4#slow6
nftset-debug yes
```

MAC rules resolve the client's source address through the local neighbor table;
MAC matches take precedence over IP rules. The client must be a reachable local
neighbor. IPv4 uses ARP; IPv6 uses the platform neighbor command. Native odhcpd
metadata and hosts-style lines supply forward and reverse records, including
multiple addresses, domain suffixes and file updates.

TCP SYN probes use Linux raw sockets and require root or `CAP_NET_RAW`. SYN/ACK
and RST replies both provide a latency measurement; the probe does not complete
a TCP connection. The existing `tcp:443` mode remains available on other systems.

Kernel `ipset` uses Linux netlink and is distinct from the internal `ip-set`
address collections. Set creation belongs to the operator/firewall. Both kernel
set integrations require the appropriate kernel support and privileges; nftset
also requires the `nft` build feature. With timeouts enabled, entries receive
three times the returned DNS TTL. Rules apply to local and cached responses as
well as upstream answers, inherit by IP family and honor rule groups, listener
overrides and ignore markers. `*-no-speed` is a fallback for a failed probe,
not for disabled/unchecked probing or an explicitly configured set.

## Encrypted DNS

```conf
bind-tls 0.0.0.0:853 -ddr
bind-https 0.0.0.0:443 -ddr
server-name resolver.home
bind-cert-generate auto
bind-cert-san resolver.home 192.168.1.1
bind-cert-validity-days 390

# For an existing encrypted PKCS#8 PEM or DER key:
# bind-cert-file /etc/smartdns/server.pem
# bind-cert-key-file /etc/smartdns/server-key.pem
# bind-cert-key-pass your-password

# Pin is the base64 SHA-256 digest of the certificate's SPKI:
# server-tls resolver.example -spki-pin BASE64_SHA256_SPKI
```

`-ddr` advertises enabled encrypted listeners through `_dns.resolver.arpa`
SVCB records, including ALPN, port, address hints and the DoH path. The discovery
target uses the configured server name and has local A/AAAA answers.
The advertised DoH URI accepts standard base64url `?dns=` GET requests and
binary POST requests; POST defaults to a binary DNS response without requiring
an `Accept` header. The existing `?name=` JSON query interface remains available.

Automatic certificate generation runs when an encrypted listener starts.
`auto` generates only when certificate/key paths are not explicitly configured;
`yes` enables generation for configured paths, while `no` disables it. A valid
existing certificate is reused. The generated leaf and local CA certificate
are stored in `smartdns-cert.pem`; the leaf key and retained root key use
`smartdns-key.pem` and `smartdns-root-key.pem` in the configuration directory.
`bind-cert-root-key-file` changes the root-key location. Clients must trust this
local CA or use an appropriate pin. Generation does not obtain a public CA
certificate. Traditional OpenSSL encrypted RSA PEM is not supported; use
encrypted PKCS#8.

The listener-specific `-ssl-certificate-key-pass` also decrypts PKCS#8 keys.
SPKI pinning checks the actual certificate public key and remains enforced with
`-no-check-certificate`; without that option normal certificate validation also
applies.

## Rust extensions and optional WebUI

Extensions use the typed `Plugin` trait in `src/plugins/mod.rs`: query-completion,
log and audit hooks receive Rust values. Register an `Arc<dyn Plugin>` during
startup. DNS response changes use the existing Rust `Middleware` trait.
Extensions are compiled with the application; loading C `.so` plugins and the
C plugin ABI are intentionally not supported.

The optional `webui` Cargo feature provides a management server implemented in
Rust, with embedded browser assets. It includes live query statistics, recent
queries, cache inspection/flush, upstream statistics, logs, DNS lookup and
configuration reload. No external plugin or web server is needed. The headless
build omits the WebUI module and assets.

See [build variants and separate Releases](BUILD_VARIANTS.md) for build commands,
configuration, package names and release tags.

## Logging and audit

`log-syslog`, `audit-SOA`, `audit-console` and `audit-syslog` are operational.
Syslog uses the local Unix logging socket. Audit syslog replaces audit-file
output; console output can be enabled separately. Audit records include SOA when
requested and measured probe latency. A single completed query is written
without waiting for a batch to fill.

`debug-save-fail-packet` and `debug-save-fail-packet-dir` save the original
malformed packet and source metadata for frontend decoding and UDP/TCP/TLS
upstream decoding. Upstream HTTP/QUIC framing is decoded inside Hickory and is
outside this diagnostic capture path.

## Verification

The test suite covers negative responses, cache budgets and refresh retries,
client/group rules, local records, kernel message encoding and query admission.
TLS tests perform real handshakes for correct/incorrect SPKI pins and encrypted
private keys. The generated certificate is also checked with a strict TLS client.

Linux raw IPv4/IPv6 SYN and kernel ipset round trips passed on the test router.
Its firmware has `CONFIG_NF_TABLES_SET` disabled, so real nftset insertion could
not be validated there. The nftset implementation now uses Rust netlink code,
with tests for address families, interval endpoints, timeouts and errors.

The DNS implementation no longer contains a C/bindgen build step. A standalone
C performance measurement utility remains under `contrib/perf`; it is not linked
into SmartDNS. Operating-system APIs and third-party crates retain their normal
platform dependencies.
