# SmartDNS-rs Optimized

中文 | [English](README_en.md)

一个面向路由器的本地 DNS 服务，支持广告过滤、域名分流和加密 DNS。本项目基于 [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs)，重点改进查询速度、资源占用和 OpenWrt 使用体验。

[下载](https://github.com/milkman-wang/smartdns-rs-optimized/releases) · [安装教程](contrib/openwrt/README.md) · [更新记录](https://github.com/milkman-wang/smartdns-rs-optimized/commits/main/)

## 能做什么

- **广告过滤与域名分流**：按域名、客户端或规则列表选择解析方式。
- **选择更快的地址**：同时查询多个上游，检测返回地址的可达性。
- **加密 DNS**：支持 UDP、TCP、DoT、DoH、DoQ 和 DoH3。
- **路由器管理**：提供中文 LuCI 页面，可选独立 WebUI。
- **本地网络解析**：支持 IPv4/IPv6、DHCP 主机名、DNS64 和地址集合规则。

## 性能对比

测试日期：2026-09-13，使用包含大量规则内存修复的最新主分支。每张表中三者均限制使用**两个 CPU 核心**，使用相同的本地上游和查询负载。以下为三轮测试的中位数，**数值越大，每秒能处理的查询越多**。

### 小米 BE10000 路由器

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 41,939 | 55,138 |
| 原作者 Rust v0.13.1 | 10,356 | 8,807 |
| 本项目（2026-09-13 主分支） | 49,640 | 115,843 |

### Ryzen 7 9800X3D · WSL2

同一台电脑的 WSL2 / Ubuntu 24.04 环境，每个 DNS 服务使用两个虚拟 CPU 核心。

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 52,231 | 113,606 |
| 原作者 Rust v0.13.1 | 24,338 | 28,403 |
| 本项目（2026-09-13 主分支） | 98,923 | 400,844 |

这些数据反映本地 DNS 的处理能力。实际体验还受公网延迟、路由器功能配置和虚拟化环境影响。单核结果、延迟、内存、具体版本及复现方法见 [详细测试报告](docs/PERFORMANCE.md)。

## 下载哪个版本

| 版本 | 适合谁 |
|---|---|
| 无界面版 | 使用 LuCI 或配置文件管理，适合日常路由器部署 |
| WebUI 版 | 希望直接在浏览器查看查询、缓存、上游和日志 |

两种版本的 DNS 功能相同。LuCI 是独立的 OpenWrt 管理页面，无界面版也能使用。

已发布的安装包见 [下载页](https://github.com/milkman-wang/smartdns-rs-optimized/releases)，近期主分支构建见 [GitHub Actions](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/build.yml)。请按设备架构和包管理器选择附件，IPK 与 APK 不能混用。上表测试程序的具体来源见详细报告。

## 开始使用

- **OpenWrt / QWRT**：按[安装教程](contrib/openwrt/README.md)安装核心、LuCI 和中文翻译，在页面中设置上游并启用服务。
- **Windows / Linux / macOS**：下载对应平台的二进制，或按[使用与配置说明](docs/USAGE.md)从源码编译。
- 程序名为 `smartdns`，OpenWrt 服务和配置名称沿用 `smartdns`。

## 更多说明

- [使用、配置与命令示例](docs/USAGE.md)
- [性能测试方法与优化说明](docs/PERFORMANCE.md)
- [LuCI 功能对照](contrib/openwrt/INTERFACE_MATRIX.md)
- [与 C SmartDNS 的兼容范围](docs/C_FEATURE_COMPATIBILITY.md)
- [构建与版本选择](docs/BUILD_VARIANTS.md)

集合写入需要固件内核支持。C 版 fallback、独立 HTTP Host 和 C 插件 ABI 尚未支持，完整差异见兼容说明。

## 来源与许可证

感谢 [mokeyish/smartdns-rs](https://github.com/mokeyish/smartdns-rs)、[C SmartDNS](https://github.com/pymumu/smartdns) 和 [Hickory DNS](https://github.com/hickory-dns/hickory-dns)。本 fork 由 milkman-wang 维护。

项目遵循 [GPL-3.0](LICENSE)，保留原作者署名；来自 Hickory DNS 的代码保留其 Apache-2.0 / MIT 许可声明。
