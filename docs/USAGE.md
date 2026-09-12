# 使用与配置

[返回首页 / Home](../README.md)

## 安装

*每日构建的版本可以在[这](https://github.com/milkman-wang/smartdns-rs-optimized/actions/workflows/nightly.yml)找到。*

- OpenWrt

  仓库内提供可交叉编译的无界面/WebUI 包、procd/UCI/dnsmasq 集成，以及 JavaScript 或 Lua CBI LuCI 页面。安装、兼容范围及验证步骤见 [OpenWrt 适配文档](../contrib/openwrt/README.md)，Rust/C 性能边界见 [OpenWrt 性能分析](../contrib/openwrt/PERFORMANCE.md)。

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
