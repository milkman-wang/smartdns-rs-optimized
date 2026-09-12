# SmartDNS-rs Optimized

![Test](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/test.yml/badge.svg?branch=main)
[![GitHub release (latest by date including pre-releases)](https://img.shields.io/github/v/release/milkman-wang/smartdns-rs-optimized?display_name=tag&include_prereleases)](https://github.com/milkman-wang/smartdns-rs-optimized/releases)
![OS](https://img.shields.io/badge/os-Windows%20%7C%20MacOS%20%7C%20Linux-blue)

[文档](docs/C_FEATURE_COMPATIBILITY.md) · [C SmartDNS 上游文档](https://pymumu.github.io/smartdns/)

[English](README.md) | 中文

SmartDNS-rs 是一个本地 DNS 服务器，可以并发查询多个上游，并选择访问更快的地址。本仓库是 **milkman-wang 维护的优化 fork**，基于 [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs)，其设计受 [C SmartDNS](https://github.com/pymumu/smartdns) 启发。

本 fork 重点改进路由器 CPU 效率、冷查询、缓存性能、Rust 原生 DNS 功能和 OpenWrt 集成。截至 **2026-09-12**，主要改动包括 UDP 连接复用、减少响应分配、受缓存预算约束的 TTL/编码复用、单 worker 运行时、ARM64 PGO 构建、可选 Rust WebUI，以及带中文翻译的 JS/Lua LuCI。保留原作者署名和许可证声明。

仓库名称为 `smartdns-rs-optimized`；可执行程序仍叫 `smartdns`，OpenWrt 软件包和服务名称保持不变。开发统一维护 **`main`**。无界面版和 WebUI 版来自同一份源码，通过构建选项区分、使用各自的 Release 标签，无需分别维护分支。

## 下载与文档

- **ARM64 PGO 0.13.1-24：**[无界面版](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-v0.13.1-r24-pgo) / [WebUI 版](https://github.com/milkman-wang/smartdns-rs-optimized/releases/tag/openwrt-webui-v0.13.1-r24-pgo)。包含 ARM64 musl IPK、二进制压缩包和 JS/Lua LuCI；不包含 APK 或其他架构。
- [更新日志](docs/releases/openwrt-0.13.1-r24-pgo.md) · [构建与版本选择](docs/BUILD_VARIANTS.md) · [OpenWrt 安装](contrib/openwrt/README.md) · [LuCI 支持矩阵](contrib/openwrt/INTERFACE_MATRIX.md)。
- [C 功能兼容说明](docs/C_FEATURE_COMPATIBILITY.md)：原生 ipset/nftset、TCP SYN 测速、SPKI、DDR、证书生成和本地记录。集合功能需要内核支持；C 版 fallback、独立 HTTP Host 和 C 插件 ABI 尚未实现。

## 性能对比

基准设备为 **小米 BE10000 / Cortex-A73、QWRT 25.12.2、Linux 5.4.213**。下表来自同批 96 次测试，各项为三轮中位数。“优化前”是本 fork 上一轮 TTL 复用版，“PGO”增加编码复用和 profile 引导编译。**这不是与未经修改的上游 SmartDNS-rs 的对比。**

| 版本 / DNS worker 数 | 冷 QPS 优化前 → PGO | 变化 | 热缓存 QPS 优化前 → PGO | 变化 |
|---|---:|---:|---:|---:|
| 无界面 / 1 | 24,784 → 30,783 | +24.2% | 57,290 → 77,412 | +35.1% |
| 无界面 / 2 | 43,766 → 53,301 | +21.8% | 105,274 → 115,564 | +9.8% |
| WebUI / 1 | 23,878 → 28,981 | +21.4% | 51,780 → 69,718 | +34.6% |
| WebUI / 2 | 42,242 → 50,324 | +19.1% | 95,517 → 114,109 | +19.5% |

测试使用本地固定上游、4,096 条缓存、256 个热域名、100 万个不循环的冷域名、32 个在途请求；关闭测速、双栈择优、预取、过期回复和审计，WebUI 版保留查询历史。冷查询逐轮核对回源计数，热吞吐测试暂停上游。96 次均零错误、零超时。普通 `cargo build --release` **不会自动获得 PGO 收益**。

另一次双发生器容量对照中，双 worker、总共 32 个在途请求达到 **132,333 QPS**，优化前为 102,010（+29.7%）。默认调度下，有线 LAN 的 P99 改善约 **4–11%**。20 秒 WebUI 持续冷查询中，最大采样 RSS 从 **16.12 降至 12.64 MiB**；小配置压测的 RSS 不等于完整路由器部署占用，也不是进程内存上限。

详见 [PGO 完整报告与复现步骤](contrib/perf/wire-pgo-20260912.md)及[原始结果](contrib/perf/wire-pgo-20260912.json)。

### C SmartDNS / OxiDNS 参考对照

此前另一批测试比较了**本 fork 的 PGO 优化前版本**、C SmartDNS Release48.4 和 OxiDNS v1.5.2 full ARM64 musl。CPU 核数表示亲和性限制，各实现保留自身线程模型。

| 实现 / CPU 核数 | 冷 QPS | 热缓存 QPS | 冷阶段末 RSS，MiB |
|---|---:|---:|---:|
| 本 fork，PGO 前 / 1 | 24,656 | 52,908 | 14.35 |
| 本 fork，PGO 前 / 2 | 43,826 | 97,648 | 15.07 |
| C SmartDNS / 1 | 17,857 | 54,776 | 6.89 |
| C SmartDNS / 2 | 40,538 | 54,580 | 6.85 |
| OxiDNS / 1 | 17,013 | 44,119 | 120.94 |
| OxiDNS / 2 | 29,777 | 80,775 | 211.41 |

结果只覆盖指定版本与简单配置。各实现的缓存淘汰语义不同，尤其 OxiDNS 的 RSS 不能推广到所有部署；也不能把此批数据与上方 PGO 数据混算为当前版本对 C/OxiDNS 的提升比例。本次没有进行 OxiDNS 与 mosdns 的运行对照。[测试方法、限制和原始数据](contrib/perf/router-efficiency-20260912.md#c--oxidns-参考批次)。

公网 RTT、首次 TLS/QUIC 建连、测速策略、大规则集和其他 SoC 都会改变结果。这里展示实现与编译的收益，不据此宣称 Rust 普遍快于 C 或 Go。

## 特性

新增配置、插件支持边界和平台要求见 [C SmartDNS 功能兼容说明](docs/C_FEATURE_COMPATIBILITY.md)。

- **多 DNS 上游服务器**

  支持配置多个上游 DNS 服务器，并同时进行查询，即使其中有 DNS 服务器异常，也不会影响查询。

- **返回最快 IP 地址**

  支持从域名所属 IP 地址列表中查找到访问速度最快的 IP 地址，并返回给客户端，提高网络访问速度。

- **支持多种查询协议**

  支持 UDP、TCP、DoT、DoQ、DoH 和 DoH3 查询及服务，以及非 53 端口查询；支持通过socks5，HTTP代理查询。

- **特定域名 IP 地址指定**

  支持指定域名的 IP 地址，达到广告过滤效果、避免恶意网站的效果。

- **域名分流**

  支持域名分流，不同类型的域名向不同的 DNS 服务器查询

- **Windows / MacOS / Linux 多平台支持**

  支持安装成服务开启自启动。

- **支持 IPv4、IPv6 双栈**

  支持 IPv4 和 IPV 6网络，支持查询 A 和 AAAA 记录，支持双栈 IP 速度优化，并支持完全禁用 IPv6 AAAA 解析。

- **支持DNS64**

  支持DNS64转换。

- **高性能、占用资源少**

  基于 [Tokio](https://tokio.rs/) 的异步 I/O；单 DNS worker 使用 current-thread 运行时，并支持缓存预算、预取及响应复用。具体性能和内存取决于配置与负载，见上方实测。

平台与功能覆盖见[兼容说明](docs/C_FEATURE_COMPATIBILITY.md)。路由器性能数据适用于已测试的 ARM64 环境。

## 安装

*每日构建的版本可以在[这](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/nightly.yml)找到。*

- OpenWrt

  仓库内提供可交叉编译的无界面/WebUI 包、procd/UCI/dnsmasq 集成，以及 JavaScript 或 Lua CBI LuCI 页面。安装、兼容范围及验证步骤见 [OpenWrt 适配文档](contrib/openwrt/README.md)，Rust/C 性能边界见 [OpenWrt 性能分析](contrib/openwrt/PERFORMANCE.md)。

- MacOS

  Homebrew 的公式安装的是上游项目，不包含本 fork 的优化。macOS 使用本 fork 时，请按下方步骤从源码构建；上游包仍可使用以下命令安装。

  ```shell
  brew update
  brew install smartdns
  ```

  注意：监听 53 端口需要 `root` 权限，因此需要 `sudo`。

  `brew` 安装的 `smartdns` 的命令 `sudo smartdns service start` 与 `sudo brew services start smartdns` 一样。

  如果没有安装 `brew`，就与下面一样，下载编译好的程序压缩包进行安装。

- Windows / Linux

  从[本 fork 的 Release](https://github.com/milkman-wang/smartdns-rs-optimized/releases)选择对应平台的附件；尚无对应附件时从源码构建。ARM64 PGO 包不能在 Windows 或 x86 上运行。解压匹配的二进制后：

  1. 查看帮助

     ```shell
     ./smartdns --help
     ```

  2. 前台运行，方便查看运行状况

     ```shell
     ./smartdns run -c ./smartdns.conf -v
     ```

     - `-v` 是开启打印调试日志

  3. 后台服务运行，开机自动运行

     查看服务管理命令：

     ```shell
     ./smartdns service --help
     ```

     *注意：安装成系统服务，需要 administrator / root 权限。*

     *服务管理是各系统兼容的，window 下调用 [sc](https://learn.microsoft.com/en-us/previous-versions/windows/it-pro/windows-server-2012-r2-and-2012/cc754599(v=ws.11))；MacOS 下调用 `launchctl` 或者 `brew`；Linux 下调用 `Systemd`  或 `OpenRc`。*

## 配置文件

下面是一份最简单的示例配置

```conf
# 在本地 53 端口监听
bind 127.0.0.1:53  

# 配置 bootstrap-dns，如不配置则调用系统的，建议配置，这样就加密了。
server https://223.5.5.5/dns-query  -bootstrap-dns -exclude-default-group

# 配置默认上游服务器
server https://dns.alidns.com/dns-query
server https://doh.pub/dns-query

# 配置公司（家里）上游服务器
server 192.168.1.1 -exclude-default-group -group office

# 以 ofc 结尾的域名转发至 office 分组进行解析
nameserver /ofc/office

# 设置域名的静态 IP
address /test.example.com/1.2.3.5

# 屏蔽域名（广告屏蔽）
address /ads.example.com/#

# 以下特性在[C 语言版 SmartDNS](https://github.com/pymumu/smartdns) 尚未支持，仅适用于SmartDNS-rs
# 使用 DoH3
server-h3 223.5.5.5

# 使用 DoQ
server-quic 223.5.5.5
```



更多高级的配置请参考 [这里](https://github.com/pymumu/smartdns/blob/doc/docs/configuration.md)

## 使用 `dig` 查询内置诊断信息

SmartDNS-rs 支持通过 `CHAOS TXT` 查询内置诊断字段。

```shell
# 最常用：一次返回完整身份信息（服务端+客户端，多条 TXT）
dig @127.0.0.1 CH TXT whoami +short

# 仅服务端身份信息（多条 TXT）
dig @127.0.0.1 CH TXT smartdns +short

# 服务器名
dig @127.0.0.1 CH TXT server-name +short

# 服务器版本
dig @127.0.0.1 CH TXT version +short

# 服务端看到的客户端源 IP
dig @127.0.0.1 CH TXT client_ip +short
dig @127.0.0.1 CH TXT client-ip +short

# 客户端 MAC（局域网且服务端 ARP 表可见）
dig @127.0.0.1 CH TXT client_mac +short
dig @127.0.0.1 CH TXT client-mac +short

# JSON 输出（后缀风格）
dig @127.0.0.1 CH TXT whoami.json +short
dig @127.0.0.1 CH TXT smartdns.json +short

# 兼容性示例
dig @127.0.0.1 CH TXT hostname.bind +short
dig @127.0.0.1 CH TXT version.bind +short
dig @127.0.0.1 CH TXT id.server +short
```

## 从源码构建与运行

假设你已经安装了 [Rust](https://www.rust-lang.org/learn/get-started)，那么你可以打开命令行界面，执行如下命令：

```shell
git clone https://github.com/milkman-wang/smartdns-rs-optimized.git
cd smartdns-rs-optimized

# 安装 https://github.com/casey/just
cargo install just

# 编译
just build --release

# 查看命令帮助
./target/release/smartdns help

# 运行
sudo ./target/release/smartdns run -c ./etc/smartdns/smartdns.conf
```

对于交叉编译，推荐使用[cross](https://github.com/cross-rs/cross)（依赖Docker）

## 鸣谢!!!

这个软件的诞生,少不了它们:

- [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs)，上游 Rust 实现
- [Hickory DNS](https://github.com/hickory-dns/hickory-dns)
- [SmartDNS](https://github.com/pymumu/smartdns)

## 开源声明

本软件包含来自 [https://github.com/hickory-dns/hickory-dns](https://github.com/hickory-dns/hickory-dns) 的代码, 其许可是下列二选一

- Apache License, Version 2.0, (LICENSE-APACHE or [](http://www.apache.org/licenses/LICENSE-2.0))
- MIT license (LICENSE-MIT or [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))

其余代码则是

- GPL-3.0 license (LICENSE-GPL-3.0 or [https://opensource.org/licenses/GPL-3.0](https://opensource.org/licenses/GPL-3.0))

## 贡献

除非您另有明确说明，否则您有意提交以包含在作品中的任何贡献，如 GPL-3.0 许可中所定义，应按上述方式获得许可，没有任何附加条款或条件。
