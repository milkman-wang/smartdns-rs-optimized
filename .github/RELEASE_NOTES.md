# SmartDNS-rs fastest-response speed check, rr-ttl caching, cache flush and LuCI updates

## 中文

### 本次更新

- `response-mode fastest-response` 配置了测速模式时，未缓存的查询仍立即返回最先到达的上游回答；同一次查询随后等待其他上游并测速，缓存改为实测最快的地址，之后的查询直接得到它。与 C 版一致，测速不会再次查询上游。
- 最多 128 个查询同时在回复后继续测速。大量不重复域名的洪泛下，超出部分按原来的 `fastest-response` 回复，内存和上游查询量不会随之增长。
- `rr-ttl-min`、`rr-ttl-max` 与 C 版一致，不只改回复里的 TTL，也决定缓存保留多久（包括否定回答和测速后的结果）。之前缓存仍按上游的原始 TTL 过期，例如设置 `rr-ttl-min 600` 时，TTL 为 20 秒的记录每 20 秒就要刷新一次。域名规则里的 `-rr-ttl` 现在生效。
- 可以不重启就清空 DNS 缓存：LuCI 高级设置里的“清空 DNS 缓存”按钮、`/etc/init.d/smartdns flushcache` 或向进程发送 `SIGUSR1`，都会立即清空缓存及其保存的副本，重启后也不会再载入旧结果；dnsmasq 转发到 SmartDNS-rs 时，同时清空 dnsmasq 的缓存。域名换了新地址时不必再等旧记录过期。
- LuCI：未写 `enabled` 的规则在界面上显示为启用，生成配置时却被跳过；现在按启用处理，表单也总是写入该选项。
- LuCI：文本文件编辑框只读取一次，没有修改时保存其他设置不再改写文件；无法读取或超过 uhttpd/rpcd 传输上限的文件改为只读，避免保存时被截断。
- 域名列表的屏蔽类型与“强制 AAAA SOA”合并为一个 `-address`（SmartDNS 只保留最后一个）；全局设置加载失败时不再生成缺少监听地址的配置。
- 规则的测速模式可填写组合；下载地址必须以 http:// 或 https:// 开头；支持原版的 `other` 下载类型和 `fatal`、`notice` 日志级别；更新文件失败时显示错误。
- LuCI（Lua 版）：只修改了自定义配置、规则列表等文本文件时，“保存并应用”会直接重新加载服务，不再提示没有需要应用的更改。
- LuCI（Lua 版）：设置、上游 DNS 服务器、规则、下载文件和日志分成独立的标签页。编辑框在较窄的屏幕上不再掉到标签下方；宽度 1152px 以下的表格不再变成每格占半行的稀疏卡片，手机上显示为紧凑卡片。
- Release 只发布内置中文的 Lua 版 LuCI。文件名带上 OpenWrt 版本（`*-openwrt-24.10*.ipk` 与 `*-openwrt-25.12*.apk`），同一版本的包排在一起。

### 升级说明

- 使用 `fastest-response` 并配置了测速模式时无需修改配置。
- 没有写 `enabled` 选项的规则现在会生效，请确认这些规则确实需要启用。
- 设置了 `rr-ttl-min` 时，缓存（包括 NXDOMAIN 等否定回答）至少保留该时长，上游记录变化后最长要等这么久才会更新。需要立即生效时可以清空 DNS 缓存。
- 装过 JS 版 `luci-app-smartdns-rs` 的设备，先卸载它和 `luci-i18n-smartdns-rs-zh-cn`，再安装与它冲突的 `luci-app-smartdns-rs-compat`；OpenWrt 24.10 及以后的官方固件需先安装 `luci-compat`。

## English

### Changes in this update

- With `response-mode fastest-response` and a speed check mode, a cache miss is still answered with the first upstream reply. The same lookup then waits for the other upstreams and probes their addresses, and the cache keeps the measured fastest address for later queries. As in C SmartDNS, no upstream is queried twice for this.
- At most 128 lookups keep probing after their answer at once. Under a flood of unique names the rest answer as plain `fastest-response`, so memory and upstream load do not grow with it.
- `rr-ttl-min` and `rr-ttl-max` now decide how long the cache keeps an answer, as in C SmartDNS, not only the TTL in replies; negative and speed-checked answers included. Previously the cache still expired answers at the upstream TTL, so with `rr-ttl-min 600` a record with a 20 s TTL was refreshed every 20 s. A domain rule's `-rr-ttl` now takes effect.
- The DNS cache can be emptied without a restart: the Clear DNS Cache button in LuCI's advanced settings, `/etc/init.d/smartdns flushcache` or `SIGUSR1` empty the cache and its saved copy at once, so a restart does not load the old answers back either. When dnsmasq forwards to SmartDNS-rs, its cache is cleared too. A domain that moved to new addresses no longer has to wait for the old records to expire.
- LuCI: rules without an `enabled` option were shown as enabled but skipped when generating the configuration. They now take effect, and the forms always write the option.
- LuCI: text file editors read each file once and leave an untouched file alone when other settings are saved. Files that cannot be read or exceed what uhttpd and rpcd can carry are shown read-only instead of being truncated on save.
- A domain list's block type and force AAAA SOA are combined into one `-address`, because SmartDNS keeps only the last one. A global section that fails part-way no longer produces a configuration without listeners.
- Rule speed check modes accept combinations, download URLs must start with http:// or https://, the original app's `other` download type and the `fatal` and `notice` log levels are supported, and file update failures are shown.
- LuCI (Lua): when only text files such as the custom configuration or rule lists changed, Save & Apply now reloads the service instead of reporting that there is nothing to apply.
- LuCI (Lua): Settings, Upstream DNS Servers, Rules, Download Files and Log are separate tabs. Editors no longer drop below their labels on narrower screens, tables narrower than 1152px no longer turn into sparse cards with every cell at half width, and phones show compact cards.
- Releases publish only the Lua LuCI app, with Chinese built in. File names carry the OpenWrt release (`*-openwrt-24.10*.ipk` and `*-openwrt-25.12*.apk`), so each release's packages sort together.

### Upgrade notes

- No configuration change is needed for `fastest-response` with a speed check mode.
- Rules saved without an `enabled` option now take effect. Check that such rules should be enabled.
- With `rr-ttl-min` set, the cache keeps answers, including NXDOMAIN and other negative answers, at least that long, so an upstream change can take that long to show. Empty the DNS cache to see it at once.
- Devices with the JavaScript `luci-app-smartdns-rs` should remove it and `luci-i18n-smartdns-rs-zh-cn`, then install `luci-app-smartdns-rs-compat`, which conflicts with it. Official OpenWrt 24.10 and later firmware needs `luci-compat` first.
