# SmartDNS-rs for OpenWrt

这里提供一套可直接放进 OpenWrt buildroot/SDK 的 `smartdns-rs` 软件包和原生 JavaScript LuCI 页面。UCI 服务名、init 脚本名和菜单路径继续使用 `smartdns`，因此可以复用原版 `luci-app-smartdns` 的配置习惯；二进制由 Rust 实现替换。

## 支持范围

- 目标：当前 OpenWrt release/snapshot，使用 `procd`、`ucode`/JavaScript LuCI 和 firewall4/nftables。
- 架构：以 OpenWrt Rust feed 的 `RUST_ARCH_DEPENDS` 为准。小闪存和 32 位低内存设备需要先核对包体与内存。
- CI 对 `aarch64_cortex-a53`、`aarch64_cortex-a72`、`aarch64_generic`、`arm_cortex-a7_neon-vfpv4`、`arm_cortex-a15_neon-vfpv4`、`i386_pentium4`、`mips_24kc`、`mipsel_24kc` 和 `x86_64` 分别运行 OpenWrt snapshot 与 24.10.7 SDK。每个架构都真正交叉编译 Rust 二进制，同时发布供 `apk` 使用的 APK 和供 `opkg` 使用的 IPK，不会只修改扩展名。
- 编译器：上游清单声明 Rust 1.96，snapshot 的 packages master 正好提供 1.96。OpenWrt 24.10 feed 当前提供 Rust 1.94；IPK 矩阵只绕过 Cargo 的清单版本门槛，仍逐架构完成真实编译。若未来上游使用 1.95/1.96 才有的语言或库 API，IPK 构建会明确失败并阻止发布。
- 默认监听 `6053`，由 dnsmasq 转发；只有显式设置端口 `53` 才停用 dnsmasq 的 DNS 端口。停止 SmartDNS-rs 后会恢复被本脚本修改的 dnsmasq 值。
- 首次安装保持禁用，先在 LuCI 检查上游和端口，再启用服务。

### 与原版 LuCI 的逐项映射

| 原版配置面 | SmartDNS-rs 映射 | 状态 |
| --- | --- | --- |
| 基本服务、UDP/TCP、IPv6、绑定设备 | `bind` / `bind-tcp`，UCI 与 procd | 完整 |
| dnsmasq 上游与 53 端口接管 | 可逆 UCI 状态保存/恢复 | 完整 |
| UDP/TCP/DoT/DoH/DoQ/DoH3 上游 | `server*` | 完整 |
| 上游组、排除默认组、EDNS、包标记、代理 | 上游参数 | 完整 |
| TLS 主机校验、SNI、跳过证书校验 | rustls 上游参数 | 完整 |
| DoT/DoH 本地监听、证书和私钥 | `bind-tls` / `bind-https` | 完整 |
| 第二监听端口 | 第二组 `bind` / `bind-tcp` 参数 | 完整 |
| 客户端规则 | `group-begin`、`client-rules`、组内规则 | 完整 |
| 域名转发、拦截、规则文件 | `domain-set` + `domain-rules` | 完整 |
| IPv4/IPv6 双栈选择开关 | 域名规则布尔值补丁 | 完整 |
| firewall4 nftset | 全局/域名规则 `nftset` | 完整 |
| IP 白/黑名单、忽略、bogus、别名 | `ip-set` 与 IP 规则 | 完整 |
| 缓存持久化、TTL、过期缓存、预取 | Rust 配置项 | 完整 |
| DHCP 主机名、resolv 文件、mDNS、DNS64 | OpenWrt 文件路径与 Rust 配置项 | 完整 |
| 日志、审计日志、自定义配置、hosts/conf 文件 | UCI + 文件编辑/包含 | 完整 |
| 规则下载和定时更新 | 原子下载、路径检查、独立 cron 标记 | 完整 |
| 内核 legacy ipset | 不生成；使用 firewall4 nftset | 不支持 |
| SmartDNS C WebUI/DDR/证书自动生成 | Rust 后端没有对应接口 | 不支持 |
| SPKI pin、HTTP Host、TCP SYN 探测、C 版 fallback | Rust 后端没有等价配置 | 不支持 |
| 旧 Lua LuCI / LEDE 17.01、OpenWrt 18.06/19.07 | 页面依赖现代 LuCI JS API | 不支持 |

LuCI 页面只显示 Rust 后端能解析和执行的选项，避免“页面保存成功、守护进程却忽略配置”。仍可在“自定义配置”页使用 SmartDNS-rs 自己支持的高级指令。

“常规设置”集中提供测速模式、响应模式和双栈 IP 优选。OpenWrt 在 UCI 未填写或留空时生成 `speed-check-mode ping,tcp:80,tcp:443` 和 `response-mode first-ping`；明确填写的值（包括测速 `none`）会保留。域名和客户端规则中的“默认”仍表示继承。仓库示例 `smartdns.conf` 也显式启用上述配置；这些是配置文件默认值，不改变 Rust 对其他手写配置的解析行为。

测速用于选择可达地址，会增加部分未缓存查询的等待时间。`first-ping` 尽快返回测速成功的地址，`fastest-ip` 等待比较，`fastest-response` 直接采用上游响应。双栈优选另行比较 IPv4/IPv6，若希望跳过这部分比较，需要单独关闭该选项。

逐个 UCI 字段及替代行为见 [INTERFACE_MATRIX.md](INTERFACE_MATRIX.md)。

## 放入 OpenWrt 源码树

```sh
git clone https://github.com/openwrt/openwrt.git
cd openwrt
./scripts/feeds update -a
./scripts/feeds install -a
cp -a /path/to/smartdns-rs/contrib/openwrt/smartdns-rs package/smartdns-rs
cp -a /path/to/smartdns-rs/contrib/openwrt/luci-app-smartdns-rs package/luci-app-smartdns-rs
make defconfig
make package/smartdns-rs/compile V=s
make package/luci-app-smartdns-rs/compile V=s
```

也可以把本目录作为自定义 feed。产物分别是：

- `smartdns-rs`：Rust 二进制、procd、UCI、dnsmasq 联动和默认文件；
- `luci-app-smartdns-rs`：LuCI 页面、ACL 和日志助手；
- `luci-i18n-smartdns-rs-zh-cn`：简体中文翻译。

若稳定版 OpenWrt 的 packages feed 还没有满足最低版本的 Rust，请使用与该 OpenWrt 分支匹配、但 Rust 足够新的 packages feed，或使用 snapshot SDK。不要用宿主机 `cargo build` 的产物代替 OpenWrt 交叉编译结果。

### Windows 本地冒烟测试包

没有 Linux SDK、Docker 或 WSL 时，可先用官方发布的 ARM64 musl 静态二进制封装真机测试包：

```powershell
python contrib/openwrt/tools/build_prebuilt_ipk.py
```

产物位于 `dist/openwrt/aarch64_cortex-a53/`。脚本固定校验官方 `v0.13.1` 归档的 SHA-256，并显式写入 OpenWrt 所需的 Unix 权限。这个途径只适合验证安装、procd、dnsmasq 和 LuCI 集成；预编译二进制不包含当前工作区尚未发布的 Rust 解析器补丁，最终发布仍应使用 SDK 构建。

## 安装与首次验证

```sh
opkg install smartdns-rs_*.ipk luci-app-smartdns-rs_*.ipk \
  luci-i18n-smartdns-rs-zh-cn_*.ipk
/etc/init.d/rpcd restart
uci set smartdns.@smartdns[0].enabled='1'
uci commit smartdns
/etc/init.d/smartdns restart
/etc/init.d/smartdns check
logread -e smartdns-rs
nslookup openwrt.org 127.0.0.1
```

apk 固件使用对应的 `apk add --allow-untrusted` 安装命令。默认情况下最后一条查询经过 dnsmasq 转发到 `127.0.0.1#6053`。生成的有效配置在 `/var/etc/smartdns/smartdns.conf`，它不是 conffile，不应手工编辑。

`check` 会重新生成配置并调用 `smartdns test`，失败时不会改动 dnsmasq。规则下载使用：

```sh
/etc/init.d/smartdns updatefiles
```

## 升级原版 luci-app-smartdns

新 init 脚本会读取原版的 `redirect` 值：`dnsmasq-upstream`、`redirect` 和 `none`，并映射到新的端口/dnsmasq 行为。先备份 `/etc/config/smartdns`，卸载原版二进制和 LuCI 包，再安装这两个包。原版独有且表中标为“不支持”的字段会保留在 UCI 中，但不会出现在新页面，也不会写入 Rust 配置。

## 开发校验

仓库包含静态契约检查和 GitHub Actions OpenWrt SDK 构建。提交前至少运行：

```sh
python3 contrib/openwrt/tests/check_contract.py
sh contrib/openwrt/tests/generate_config.sh
sh contrib/openwrt/tests/dnsmasq_state.sh
shellcheck -s sh contrib/openwrt/smartdns-rs/files/etc/init.d/smartdns \
  contrib/openwrt/smartdns-rs/files/etc/uci-defaults/90-smartdns-rs \
  contrib/openwrt/luci-app-smartdns-rs/root/usr/libexec/smartdns-rs-call \
  contrib/openwrt/tests/generate_config.sh \
  contrib/openwrt/tests/dnsmasq_state.sh
node --check contrib/openwrt/luci-app-smartdns-rs/htdocs/luci-static/resources/view/smartdns/smartdns.js
```

性能边界和建议的设备实测方法见 [PERFORMANCE.md](PERFORMANCE.md)。

## Fork 自动同步和发布

Fork 的 `Sync upstream` 工作流每天检查一次 `mokeyish/smartdns-rs` 的 `main` 分支。发现新提交时会：

1. 用普通 Git merge 合并上游，遇到冲突立即失败，不强行覆盖 OpenWrt 适配；
2. 更新 OpenWrt Makefile 固定的上游提交、源码 SHA-256 和 Cargo 版本；
3. 推送 Fork 的 `main` 并调度九架构的 APK/IPK 双格式构建；
4. 仅在契约测试、九个 snapshot APK 架构和九个 OpenWrt 24.10.7 IPK 架构全部成功后创建 GitHub Release，同时生成 `SHA256SUMS`。Release 只包含本项目的主包、LuCI 和简体中文翻译，不包含 SDK 下载的依赖包；APK 主包文件名会附加 OpenWrt 架构，避免不同架构相互覆盖。发布文件名中的 `~` 会规范为 `.`，与 GitHub 实际保存的附件名及校验清单保持一致。

Action 的运行编号会成为单调递增的 `PKG_RELEASE`，因此同一个 SmartDNS-rs 版本内的后续自动包也能被包管理器识别为升级。OpenWrt 25.12/snapshot 使用 APK；OpenWrt 24.10 以及仍使用 opkg 的兼容固件应安装 Release 中的 IPK。两个矩阵使用各自的官方 SDK 构建，包格式不能混装。

GitHub 默认会禁用新 Fork 的定时工作流，首次创建 Fork 后需要在 Actions 页面启用一次。也可以手动运行 `Sync upstream` 或 `OpenWrt packages` 工作流。
