# 原版 LuCI 接口对照

对照基线是 `pymumu/luci-app-smartdns` 的提交 [`581e5e8`](https://github.com/pymumu/luci-app-smartdns/commit/581e5e816d92d3a663b1b2e331e3f21685968cf1)。“支持”表示 LuCI 字段、UCI 读取、生成的 Rust 指令和运行时行为四层都已接通；仅在页面上放一个同名字段不算支持。

## 全局与监听

| 原版 UCI 字段 | SmartDNS-rs 处理 | 状态 |
| --- | --- | --- |
| `enabled` | procd 实例开关 | 支持 |
| `server_name` | `server-name` | 支持 |
| `port` | UDP/TCP `bind`；6053 转发或 53 接管 | 支持 |
| `auto_set_dnsmasq` | 保存、修改并恢复 dnsmasq UCI | 支持 |
| 旧 `redirect` | `dnsmasq-upstream` / `redirect` / `none` 兼容读取 | 迁移兼容 |
| `tcp_server`, `ipv6_server` | `bind-tcp` 与双栈监听 | 支持 |
| `bind_device`, `bind_device_name` | `@device` 监听 | 支持 |
| `server_flags` | 追加到监听器；由 `smartdns test` 最终校验 | 支持 |
| `tls_server`, `tls_server_port` | `bind-tls` | 支持 |
| `doh_server`, `doh_server_port` | `bind-https` | 支持 |
| `bind_cert`, `bind_cert_key`, `bind_cert_key_pass` | Rust TLS 证书字段 | 支持 |
| `seconddns_enabled`, `seconddns_port`, `seconddns_tcp_server` | 第二 UDP/TCP 监听器 | 支持 |
| `seconddns_server_group` | `-group` | 支持 |
| `seconddns_no_speed_check` | `-no-speed-check` | 支持 |
| `seconddns_no_rule_addr`, `seconddns_no_rule_nameserver`, `seconddns_no_rule_ipset`, `seconddns_no_rule_soa` | 对应 bind 参数 | 支持 |
| `seconddns_no_dualstack_selection`, `seconddns_no_cache` | 对应 bind 参数 | 支持 |
| `seconddns_force_aaaa_soa`, `seconddns_force_https_soa` | 对应 bind 参数 | 支持 |
| `seconddns_ipset_name`, `seconddns_nftset_name`, `seconddns_no_serve_expired` | 监听器集合与过期缓存参数 | 支持 |
| `seconddns_no_ip_alias` | Rust 监听器没有对应字段 | 不支持 |

## 解析、缓存和本机数据

| 原版 UCI 字段 | SmartDNS-rs 处理 | 状态 |
| --- | --- | --- |
| `speed_check_mode` | `speed-check-mode` | 支持 |
| `response_mode` | `response-mode` | 支持 |
| `dualstack_ip_selection` | `dualstack-ip-selection` | 支持 |
| `prefetch_domain`, `serve_expired` | 同名 Rust 指令 | 支持 |
| `cache_size`, `cache_persist`, `cache_file` | 缓存和持久化 | 支持 |
| `rr_ttl`, `rr_ttl_min`, `rr_ttl_max`, `rr_ttl_reply_max` | 对应 TTL 指令；同时修复规则合并时 `rr_ttl_max` 误取 min 的问题 | 支持 |
| `resolve_local_hostnames` | OpenWrt DHCP lease 文件 | 支持 |
| `mdns_lookup` | Rust mDNS | 支持 |
| `force_aaaa_soa`, `force_https_soa` | `force-qtype-SOA` | 支持 |
| `dns64` | `dns64` | 支持 |
| `client_addr_file` | Rust 没有原版文件加载语义；使用多值 `client_addr` | 替代实现 |
| `ddr` | 加密监听器 `-ddr`，提供 DDR 发现记录 | 支持 |
| `num_workers`, `cache_mem_size`, `max_query_limit` | 工作线程、缓存内存预算、并发查询上限 | 支持 |
| `serve_expired_ttl`, `serve_expired_reply_ttl`, `serve_expired_prefetch_time` | 过期缓存保留、回复 TTL 和刷新间隔 | 支持 |
| `domain`, `odhcpd_lease_file` | 本地域名后缀、odhcpd 租约文件 | 支持 |

## 上游 DNS

| 原版 UCI 字段 | SmartDNS-rs 处理 | 状态 |
| --- | --- | --- |
| `enabled`, `name`, `ip`, `port` | 上游行；`name` 仅作 LuCI 标签 | 支持 |
| `type` | `udp`, `tcp`, `tls`, `https`, `quic`, `h3` | 支持 |
| `server_group` | `-group`；已补充分组名连字符解析 | 支持 |
| `exclude_default_group` | `-exclude-default-group` | 支持 |
| `blacklist_ip`, `check_edns` | 对应上游过滤参数 | 支持 |
| `tls_host_verify`, `host_name`, `no_check_certificate` | TLS 校验与 SNI | 支持 |
| `set_mark` | `-set-mark`；LuCI 同时校验十进制和 `0x` 十六进制 32 位标记 | 支持 |
| `use_proxy` | 引用 `default-proxy` | 支持 |
| `addition_arg` | 高级参数透传并在启动前验证 | 支持 |
| `fallback` | Rust 上游模型没有 C 版 fallback 标记 | 不支持 |
| `spki_pin` | 上游 `-spki-pin` | 支持 |
| `http_host` | Rust DoH URL/SNI 模型没有独立 HTTP Host 字段 | 不支持 |

## 客户端、域名和 IP 规则

| 原版 UCI 字段 | SmartDNS-rs 处理 | 状态 |
| --- | --- | --- |
| `client_addr`, `server_group` | `client-rules` + 组内 `nameserver` | 支持 |
| 客户端 `speed_check_mode`, `dualstack_ip_selection`, `force_aaaa_soa` | 组内 `domain-rules /./` | 支持 |
| 客户端/域名 `nftset_name` | 组内或域名 `-nftset`；支持集合名连字符 | 支持 |
| `domain_forwarding_list`, `domain_block_list` | 固定 conffile 文本编辑 | 支持 |
| `forwarding_domain_set_file`, `block_domain_set_file` | `domain-set -file` | 支持 |
| 规则列表 `domain_list_file`, `server_group`, `block_domain_type` | 独立 `domain-set` + `domain-rules` | 支持 |
| 域名规则/规则列表 `force_aaaa_soa` | `-address #6`；已在两个 LuCI 界面补齐 | 支持 |
| 规则列表 `speed_check_mode`, `dualstack_ip_selection`, `nftset_name`, `addition_flag` | 域名规则参数 | 支持 |
| `whitelist_ip`, `blacklist_ip`, `ignore_ip`, `bogus_nxdomain` | IP 地址/文件集合规则 | 支持 |
| `ip_alias` | `ip-alias` | 支持 |
| `ipset_name`, `ipset_no_speed` | 原生 Rust Netlink 写入 legacy ipset | 支持；需要内核支持 |
| `nftset_no_speed`, `nftset_timeout`, `nftset_debug`, `ipset_timeout` | 集合写入与调试选项 | 支持；需要内核支持 |
| 客户端/域名/规则列表 `ipset_name`, `no_serve_expired` | 集合与过期缓存规则 | 支持 |

## 文件、日志和 C 版专属页面

| 原版 UCI/页面字段 | SmartDNS-rs 处理 | 状态 |
| --- | --- | --- |
| `conf_files`, `hosts_files` | 多文件 `conf-file` / `hosts-file` | 支持 |
| `upload_conf_file`, `upload_list_file`, `upload_other_file` | 分别上传到 `conf.d`、`domain-set` 和 `/etc/smartdns`，可用于规则、证书及其他文件 | 支持 |
| 上传与下载规则文件 | 安全文件名、原子替换、失败聚合、可选代理；文件名允许 `.txt` 等正常扩展名 | 支持 |
| 下载项 `desc` | 作为 LuCI 说明元数据保留，不写入运行配置 | 支持 |
| `enable_auto_update`, `auto_update_week_time`, `auto_update_day_time` | 仅维护带 SmartDNS-rs 标记的 cron 行 | 支持 |
| `log_level`, `log_file`, `log_size`, `log_num` | 文件日志 | 支持 |
| `enable_audit_log`, `audit_log_file`, `audit_log_size`, `audit_log_num` | 文件审计日志 | 支持 |
| `log_output_mode`, `audit_log_output_mode` | 不复用 C 版枚举；使用 `log_syslog`、`audit_console`、`audit_syslog` 和 `audit_soa` | 替代实现 |
| `debug_save_fail_packet`, `debug_save_fail_packet_dir` | 保存解析失败的数据包 | 支持 |
| `address_conf`, `blackip_ip_conf`, `custom_conf` | LuCI 文件编辑器；不写虚拟 UCI 值 | 支持 |
| `coredump` | procd core limit；实际落盘位置遵循内核 `core_pattern`，LuCI 不再错误承诺固定目录 | 支持 |
| `ui`, `web`, `ui_port`, `ui_data_dir`, `ui_log_max_age` | C 版内嵌 WebUI 专属 | 不支持 |
| `view_log` | 独立日志页面及清空操作 | 替代实现 |
| `bind_cert_generate`, `bind_cert_san`, `bind_cert_validity_days`, `bind_cert_root_key_file` | 加密监听器证书生成 | 支持 |
| `webui_enable`, `webui_bind` | 独立 Rust WebUI；需安装 WebUI 核心包 | 支持 |
| `report` / `Donate` 技术支持与捐助入口 | 不属于 DNS 运行接口 | 未移植 |

JS 页面保存、文件写入和命令执行由独立 rpcd ACL 限制。Lua 兼容版通过登录后的 CBI 表单处理写入和操作按钮，现代 LuCI 另有菜单 ACL；两者使用相同的日志助手。服务日志必须直接位于 `/var/log/smartdns/`，缓存文件必须直接位于 `/etc/smartdns/` 且以 `.cache` 结尾；日志助手解析符号链接后的真实路径后才允许读取普通文件。下载文件名拒绝隐藏路径、斜杠和反斜杠。

## LuCI 有效性与翻译审计

- 两个 LuCI 页面的静态界面文本均有非空 `zh_Hans` 翻译；契约测试检查实际覆盖率，避免文案增加后漏翻译。测速模式接受 `tcp-syn:端口`，缓存条目数接受 `-1`、`0` 和正整数。
- 原版中后端不支持的字段不会显示成“可保存但无效果”的控件；上表标为“不支持”的功能只能通过将来补充 Rust 后端能力后再开放。
- `server.name`、规则列表的 `name` 和下载项 `desc` 只是 LuCI 展示元数据，不参与 SmartDNS-rs 运行配置；文件编辑器、即时上传和操作按钮则直接读写受 ACL 限制的文件或调用 init 动作，因此也不会生成虚假的 UCI 指令。
- 下载文件名不再使用错误的 `uciname` 校验，`antiad.txt`、`hosts.txt` 等带扩展名的安全文件名可以正常保存；上游数据包标记与 Rust 一致，接受 32 位十进制和十六进制值。
- 契约测试会同时检查 Rust 指令、LuCI 字段消费关系、动态 gettext、简体中文覆盖率以及上述两个输入校验，防止以后再次出现无效 UI 或漏翻译。

## Lua CBI 兼容页面

`luci-app-smartdns-rs-compat` 覆盖上面的有效 UCI 配置项，继续调用同一个 Rust 配置生成器。现代 LuCI 需要额外安装 `luci-compat`，旧 Lua LuCI 直接使用内置 CBI。两套页面包不能同时安装。

Lua 版用路径输入替代 JS 文件选择器；在“文件和更新”中填写文件名并上传后，可将完整路径填入规则项。日志通过刷新页面更新。配置按钮操作已保存的配置，先保存并应用再校验或重启。契约检查会检查 Lua 字段覆盖与翻译，Lua 5.1 测试验证默认值、输入校验、文件写入/上传和命令处理。
