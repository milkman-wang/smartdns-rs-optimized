# SmartDNS-rs recheck of local answers, C-style rule inheritance, smaller binary and security updates

## 中文

### 本次更新

- 新增域名规则选项 `-recheck-nameserver` 与 `-recheck-ip-set`（仅 SmartDNS-rs 支持）：不在名单里的域名先交给远程 DNS，国外域名不会发给国内 DNS；回答里有地址落在指定 ip-set（例如 chnroute）时，再向国内分组查一次，名单之外的国内网站也能拿到最近的节点。第二次的回答按全局的 `speed-check-mode` 与 `response-mode` 测速选址。Passwall 默认 DNS 选远程时，在 custom.conf 加一行 `domain-rules /./ -recheck-nameserver China -recheck-ip-set chnroute` 即可，同一行可加 `-subnet` 让远程 DNS 按你所在地区回答。详见使用说明。
- 域名规则与 C 版一样逐项继承上级规则：自己没写的选项（分组、测速、双栈、`-address` 等）沿用最近一条写了它的上级规则，`/./` 也算上级；`rr-ttl` 三项只整体继承。之前根规则从未成为上级，名单规则也不继承任何选项。
- 规则查找不再为每次查询构造域名，加载 Passwall 名单时未缓存查询快约 5%、缓存命中快约 9%。
- `address /域名/#` 与 `-address #` 与 C 版一致，对所有查询类型都回答 NXDOMAIN。之前只拦截 A/AAAA，HTTPS、SVCB 等仍会转发上游，其中的 IP 提示可能让被屏蔽的广告服务器照样连上。
- 没有配置 `server`、系统 resolv.conf 又指向 SmartDNS-rs 自己（OpenWrt 上监听 53 端口时就是如此）时，查询不再转发给自己无限循环、直到内存耗尽；指向自身监听地址的上游会被跳过并记录警告，必要时改用 AliDNS 与 Cloudflare 的 DoH。
- 兼容 luci-app-smartdns 生成的配置：文件路径可以用单引号（之前广告列表等 domain-set 因此加载失败）；同一域名分多行写的 `-subnet`、`-cname` 现在都会生效。
- 更新依赖，修复 13 个影响 OpenWrt 版本的安全公告，涉及 rustls、rustls-webpki、webpki-roots、quinn-proto（DoQ/DoH3）、h2 等；tokio 与缓存使用的 lru 的不安全问题也已修复。OpenWrt 24.10 SDK 的 Rust 1.94 仍可编译。
- 程序体积缩小约 17%（aarch64 OpenWrt 版 10.1 MB → 8.4 MB），用少量自写代码代替了只用到一小部分的依赖，依赖库按体积优化；没有删减任何功能，查询性能基本不变。

### 升级说明

- 被 `#` 屏蔽的域名现在对所有类型回答 NXDOMAIN，不再需要另开解析器拒绝 HTTPS/SVCB。
- 规则继承后，上级规则（尤其是 `/./`）写的选项会作用到没写这些选项的下级规则和名单，请确认符合预期。
- 重查的回答要测速时需在全局写 `speed-check-mode`，例如 C 版默认的 `speed-check-mode ping,tcp:80,tcp:443`；SmartDNS-rs 不写时不测速。

## English

### Changes in this update

- New domain rule options `-recheck-nameserver` and `-recheck-ip-set` (SmartDNS-rs only). Names outside your lists go to a remote resolver, so foreign names never reach the local one; when an answer has an address in the given ip-set (chnroute, for example), the local group is asked once more, so local sites missing from the lists still get their nearest servers. The second answer is speed-checked by the global `speed-check-mode` and `response-mode`. With Passwall's default DNS set to remote, add `domain-rules /./ -recheck-nameserver China -recheck-ip-set chnroute` to custom.conf; a `-subnet` on the same line makes the remote resolver answer for your region. See the usage guide.
- Domain rules inherit from parent rules option by option, as in C SmartDNS: an option a rule does not set (group, speed check, dual-stack, `-address` and so on) comes from the nearest parent rule that sets it, `/./` included; the three `rr-ttl` options are inherited only together. Previously the root rule was never a parent, and list rules inherited nothing.
- Rule lookups no longer build names for each query: with Passwall's lists, uncached queries are about 5% faster and cache hits about 9% faster.
- `address /name/#` and `-address #` answer NXDOMAIN for every query type, as in C SmartDNS. Previously only A and AAAA were blocked; HTTPS, SVCB and other types still went upstream, and their address hints could let a blocked ad server be reached anyway.
- With no `server` configured and the system resolv.conf naming SmartDNS-rs itself, as on OpenWrt with SmartDNS-rs on port 53, queries are no longer sent back to itself in a loop until memory runs out. Upstreams that are one of its own listeners are skipped with a warning, and AliDNS and Cloudflare over HTTPS stand in when needed.
- Configurations from luci-app-smartdns work as they do with C SmartDNS: file paths may be single-quoted (domain sets such as ad lists failed to load before), and `-subnet` and `-cname` in domain rules take effect, also when written on a separate line for the same name.
- Dependencies are updated, fixing 13 advisories that affected the OpenWrt build, in rustls, rustls-webpki, webpki-roots, quinn-proto (DoQ, DoH3), h2 and more; unsoundness in tokio and in the cache's lru is fixed too. The Rust 1.94 of the OpenWrt 24.10 SDK still builds it.
- The binary is about 17% smaller (aarch64 OpenWrt build 10.1 MB to 8.4 MB): a little code of its own replaces dependencies that were barely used, and dependencies are optimized for size. No feature is removed, and query performance is about the same.

### Upgrade notes

- Names blocked with `#` now get NXDOMAIN for every type, so no extra resolver is needed to refuse HTTPS and SVCB.
- With inheritance, options set on parent rules, `/./` in particular, now apply to child rules and lists that do not set them. Check that this is what you want.
- To speed-check rechecked answers, set `speed-check-mode` globally, for example to the C SmartDNS default `speed-check-mode ping,tcp:80,tcp:443`; SmartDNS-rs checks nothing without it.
