# SmartDNS-rs 0.13.1-24 ARM64 PGO

发布日期：2026-09-12。无界面版与 WebUI 版分别发布；核心 Rust 版本保留 0.13.1，OpenWrt 包修订号升级为 24。

## 变化

- 优化单线程与多线程查询路径、上游 UDP 连接复用、缓存回复编码复用，减少分配和重复序列化。复用的数据纳入已有缓存内存预算。
- 使用真实 ARM64 路由器工作负载训练 PGO，并重新编译 headless 和 WebUI 两种程序。发布程序不包含 profile 收集插桩。
- 增加 Rust 原生 ipset/nftset Netlink、TCP SYN 测速、SPKI pin、DDR、证书生成、odhcpd、本地域名与 TXT 记录等能力；补充缓存预算、过期缓存、日志与查询控制。
- 提供可选的独立 Rust WebUI，包括运行概览、查询、缓存、上游、日志和配置管理。无界面版不编译管理页面。
- JS 与 Lua CBI LuCI 同步适配线程数、缓存内存预算、并发上限、过期缓存参数、ipset、SPKI、DDR、证书生成、odhcpd、syslog、失败包调试和 WebUI 配置，补齐中文翻译。TXT 等高级指令可使用原生配置编辑器。
- 修复 LuCI 缓存条目数对 `-1` 的校验，支持 `tcp-syn:端口`，补充监听器、客户端和域名规则的集合及过期缓存设置。

## 性能与验证

以 BE10000 的 Cortex-A73、QWRT/Linux 5.4 为基准，与前一轮优化产物相比：无界面版单线程冷查询吞吐约提升 24%，单线程热查询约提升 35%；双线程热缓存容量测试约提升 30%。WebUI 冷查询持续负载 RSS 从约 16.1 MiB 降至 12.6 MiB。

这些是指定配置和负载下的测量结果，不代表所有设备、公网域名或测速策略都有同等收益；公网冷查询仍受上游与网络延迟影响。详见[测试方法和原始数据](../../contrib/perf/wire-pgo-20260912.md)。优化不依赖路由器 NSS/PPE DNS 硬件卸载。

核心测试 351 项通过、1 项忽略，Windows/ARM Clippy 与格式检查通过；另外验证 OpenWrt 配置生成、dnsmasq 恢复、LuCI 字段/翻译契约、Lua 表单以及两种 IPK 内容。

## 安装与边界

- 本次附件为 `aarch64_cortex-a53` 元数据的 ARM64 musl IPK 和二进制压缩包，以及通用 LuCI 包。没有提供其他架构或 APK；使用 opkg 固件安装，不能把 IPK 当作 APK。
- `smartdns-rs` 与 `smartdns-rs-webui` 二选一；JS LuCI 与 Lua compat LuCI 也二选一，安装对应中文包。升级前备份 `/etc/config/smartdns` 与 `/etc/smartdns`。
- WebUI 配置需要 WebUI 核心包。沿用旧配置不会自动打开管理端口；启用方式见[构建说明](../BUILD_VARIANTS.md)。
- ipset/nftset 取决于内核支持。本次基准固件支持 legacy ipset，未启用 nftables 集合，软件升级不能补齐固件内核能力。
- C 版 fallback、独立 HTTP Host、监听器 `-no-ip-alias` 和 C 插件 ABI 尚未实现；没有为这些功能添加无效的 LuCI 控件。
