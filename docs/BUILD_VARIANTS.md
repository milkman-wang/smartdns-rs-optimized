# 两种构建与两条 Release

DNS 核心和 WebUI 后端均使用 Rust 实现。无界面版不编译内置管理页面，
WebUI 版通过 `webui` Cargo feature 加入管理模块和页面资源；两者使用相同的 DNS 功能。
不加载 C 插件，也不需要额外安装 Web 服务。

| | Headless 无界面版 | WebUI 版 |
| --- | --- | --- |
| Cargo | 默认构建 | 默认构建加 `webui` |
| 通用压缩包目录 | `smartdns-<target>` | `smartdns-webui-<target>` |
| 通用 Release 标签 | `v<version>` | `webui-v<version>` |
| OpenWrt 核心包 | `smartdns-rs` | `smartdns-rs-webui` |
| OpenWrt Release 标签 | `openwrt-v<version>-r<build>` | `openwrt-webui-v<version>-r<build>` |

两个 Release 独立发布、独立更新附件，不会互相覆盖。自更新只选择当前构建类型的
Release 和目标平台压缩包，不会把 WebUI 版换成无界面版。发布构建从
`SMARTDNS_RELEASE_REPOSITORY` 读取所属仓库；未设置时使用 Cargo 清单中的仓库地址。

## 构建

```sh
# 通用版本
cargo build --locked --release
cargo build --locked --release --features webui

# OpenWrt 功能集合
cargo build --locked --release --no-default-features --features openwrt
cargo build --locked --release --no-default-features --features openwrt,webui

# 各自在对应构建完成后打包
just flavor=headless target=<target> pack
just flavor=webui target=<target> pack
```

执行 `smartdns --version` 可看到 `headless` 或 `webui`，方便确认安装的版本。
同一目录连续构建时，应先保存第一种构建的程序，再构建另一种；两种程序都叫 `smartdns`。

## WebUI 使用

通用 WebUI 版默认监听 `127.0.0.1:6080`。可以在配置文件中设置：

```conf
webui-enable yes
webui-bind 127.0.0.1:6080
```

浏览器打开对应地址，即可查看运行概览、最近查询、DNS 缓存、上游服务器、日志和服务设置。
查询记录保留最近 1,000 条，日志保留最近 300 条；这两项记录在内存中，重启后清空。
缓存页展示最近使用的最多 1,000 条缓存，同时显示实际缓存总数。
清空缓存与重新加载配置直接调用当前服务；修改管理端口或启停 WebUI 需要重启服务。

管理页面仅在独立的 WebUI 监听端口提供，不会自动挂到对外的 DoH 地址上。
无界面版没有 WebUI 监听端口；显式配置 `webui-enable yes` 会提示安装 WebUI 版。

## OpenWrt

SDK 使用原生包变体分别编译 `headless` 和 `webui`，并使用各自的构建目录。
两个核心包提供相同的 `/usr/sbin/smartdns`、procd 服务和 UCI 配置，因此互斥安装。
切换版本时保留已有的 `/etc/config/smartdns` 和 `/etc/smartdns/` 配置。
切回无界面版前，将 `webui_enable` 设为 `0` 或删除该项，避免旧配置继续要求启动 WebUI。
LuCI 包仍是独立、可选的 OpenWrt 管理接口，JS 和 Lua compat 版本继续提供。

全新安装 WebUI 核心包的默认配置包含：

```uci
option webui_enable '1'
option webui_bind '0.0.0.0:6080'
```

启动服务后，通过路由器 LAN 地址的 6080 端口访问。如果沿用旧 UCI 配置，需自行补上这两项，
或指定希望使用的管理地址。通用二进制的默认地址仍是本机回环地址。

本地 IPK 测试包可以使用已编译的目标程序生成：

```sh
python3 contrib/openwrt/tools/build_prebuilt_ipk.py --binary <headless-binary> --variant headless --output dist/headless
python3 contrib/openwrt/tools/build_prebuilt_ipk.py --binary <webui-binary> --variant webui --output dist/webui
```

`--variant webui` 必须传入带 `webui` feature 的程序。该脚本只打包，不替你编译。

## 发布流程

- `openwrt.yml`：无界面版 OpenWrt 构建和 Release。
- `openwrt-webui.yml`：WebUI 版 OpenWrt 构建和 Release。
- `openwrt-build.yml`：共享 SDK 步骤，每个版本各自等待九种架构的 APK/IPK 构建完成后发布。
- `build.yml`：通用平台构建，`variant` 输入选择类型。
- `version.yml`：版本更新后启动两个独立的通用构建，产生两个 Release。
- `nightly.yml`：分别提供两种 nightly 构建附件。

提交这些工作流本身不会创建本地改动的 Release；GitHub 必须先取得已提交的源码并实际运行构建。

### ARM64 PGO 发布

`openwrt-v0.13.1-r24-pgo` 和 `openwrt-webui-v0.13.1-r24-pgo` 是独立的 ARM64 musl PGO 发布，包版本为 `0.13.1-24`。它们包含实际交叉编译的核心 IPK、JS/Lua LuCI 及中文翻译；不是九架构 SDK 构建，也不提供 APK。PGO 使用路由器上的单线程与双线程 DNS 工作负载训练，不限定到 Cortex-A73 指令集。构建与实测依据见 [性能报告](../contrib/perf/wire-pgo-20260912.md)，功能与限制见 [更新日志](releases/openwrt-0.13.1-r24-pgo.md)。
