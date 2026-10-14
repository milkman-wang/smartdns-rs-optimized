# SmartDNS-rs fastest-response speed check and LuCI fixes

## 中文

### 本次更新

- `response-mode fastest-response` 配置了测速模式时，未缓存的查询仍立即返回最先到达的上游回答；同一次查询随后等待其他上游并测速，缓存改为实测最快的地址，之后的查询直接得到它。与 C 版一致，测速不会再次查询上游。
- 最多 128 个查询同时在回复后继续测速。大量不重复域名的洪泛下，超出部分按原来的 `fastest-response` 回复，内存和上游查询量不会随之增长。
- LuCI：未写 `enabled` 的规则在界面上显示为启用，生成配置时却被跳过；现在按启用处理，表单也总是写入该选项。
- LuCI：文本文件编辑框只读取一次，没有修改时保存其他设置不再改写文件；无法读取或超过 uhttpd/rpcd 传输上限的文件改为只读，避免保存时被截断。
- 域名列表的屏蔽类型与“强制 AAAA SOA”合并为一个 `-address`（SmartDNS 只保留最后一个）；全局设置加载失败时不再生成缺少监听地址的配置。
- 规则的测速模式可填写组合；下载地址必须以 http:// 或 https:// 开头；支持原版的 `other` 下载类型和 `fatal`、`notice` 日志级别；更新文件失败时显示错误。

### 升级说明

- 使用 `fastest-response` 并配置了测速模式时无需修改配置。
- 没有写 `enabled` 选项的规则现在会生效，请确认这些规则确实需要启用。

## English

### Changes in this update

- With `response-mode fastest-response` and a speed check mode, a cache miss is still answered with the first upstream reply. The same lookup then waits for the other upstreams and probes their addresses, and the cache keeps the measured fastest address for later queries. As in C SmartDNS, no upstream is queried twice for this.
- At most 128 lookups keep probing after their answer at once. Under a flood of unique names the rest answer as plain `fastest-response`, so memory and upstream load do not grow with it.
- LuCI: rules without an `enabled` option were shown as enabled but skipped when generating the configuration. They now take effect, and the forms always write the option.
- LuCI: text file editors read each file once and leave an untouched file alone when other settings are saved. Files that cannot be read or exceed what uhttpd and rpcd can carry are shown read-only instead of being truncated on save.
- A domain list's block type and force AAAA SOA are combined into one `-address`, because SmartDNS keeps only the last one. A global section that fails part-way no longer produces a configuration without listeners.
- Rule speed check modes accept combinations, download URLs must start with http:// or https://, the original app's `other` download type and the `fatal` and `notice` log levels are supported, and file update failures are shown.

### Upgrade notes

- No configuration change is needed for `fastest-response` with a speed check mode.
- Rules saved without an `enabled` option now take effect. Check that such rules should be enabled.
