# 单核效率、冷查询与 OxiDNS 实现研究（2026-09-12）

本轮在上一轮已完成的优化上继续改进。最终无 WebUI 版热缓存吞吐单核提高 8.0%、双核提高 7.8%；单核冷查询提高约 2.0%，双核持平。WebUI 版热缓存提高 5.8% / 2.8%，冷查询基本持平。低负载与固定 1000 QPS 的 P99 并未全面改善，不能将吞吐收益表述为所有场景延迟下降。

数据来自 BE10000 / QWRT 25.12.2、Linux 5.4.213 AArch64。它是固定基准设备；实现没有依赖该型号、调频修改或专用指令。Windows x86-64 用于回归测试，其他 SoC 的性能比例仍需实测。

## 保留的实现与代码取舍

- `num-workers 1` 使用 Tokio current-thread 运行时，多 worker 保持多线程。单 worker 表示一个 DNS 事件循环，阻塞操作仍可能使用辅助线程；不宣称进程只有一个 OS 线程。
- 缓存的 8 个方法内部已经没有异步等待，改为同步函数，移除调用处多余的 `.await`，保持已有锁和淘汰方式。
- 同一条目在相同整秒剩余 TTL 下第二次命中后，保留一次已调整 TTL 的响应。后续命中共享它，减少重复记录复制和 TTL 改写。TTL 改变时，下次访问释放旧副本；原始响应一直保留，供持久化、导出和过期回复使用。
- 额外副本计入已有缓存内存预算；预算不足就不保留，不在命中路径驱逐别的条目。稀疏访问不主动创建副本，先前热门但已闲置的副本随下一次 TTL 变化访问、淘汰或清空释放。内存预算是缓存对象估算值，不是整个进程 RSS 上限。
- 移除仅使用单一类型的缓存条目泛型。没有新增依赖、协议解析器、配置开关或硬件分支。

仅更换单线程运行时的独立实验，单核冷查询约 +0.5%、热缓存约 +1.9%，低负载 P99 没有改善。清理同步 API 后冷查询有小幅收益。尝试直接读取 JoinSet 已完成任务未带来稳定收益，已撤回。TTL 复用的初筛给出约 8%–9% 热缓存收益后，再收紧到重复命中才保留；下面全部使用最终产物重新测量，不混用初筛成绩。

## 最终同批对照

每个组合 3 轮，共 96 次；表中为各指标三轮中位数，不是将原始延迟样本合并后的分位数。`before` 是本轮开始时的上一轮最终版，`final` 是本轮改进版，均为 release / thin LTO。数据与逐轮范围见 [原始 JSON](router-efficiency-20260912.json)。

单核：1 worker 绑定 CPU1；双核：2 workers 绑定 CPU1–2。客户端固定 CPU0、模拟上游 CPU3。C 对照保留其线程模型，只约束同等 CPU 亲和性。CPU governor 保持 ondemand。

配置：缓存 4096 条，热集 256 域名；关闭预取、过期应答、测速、双栈择优和审计，WebUI 变体启用内置管理界面。预热 1 秒；吞吐测 5 秒、窗口 32；100 / 1000 QPS 各测 10 秒、窗口 4。上游返回固定 A 记录、TTL 3600。它隔离本地 DNS 开销，不能预测公网 DNS 往返或首次 TLS/QUIC 建连耗时。

冷查询使用 100 万域名，计时内未循环；每轮上游计数严格等于成功数减 255（预热交集）。热吞吐期间暂停模拟上游，全部命中且上游计数为零。96 次全部零错误、零超时。

| 版本 / CPU | 冷 QPS 前 → 后 | 变化 | 热 QPS 前 → 后 | 变化 | 热查询 CPU μs/次 前 → 后 |
|---|---:|---:|---:|---:|---:|
| 无 WebUI / 1 | 24,677 → 25,165 | +2.0% | 52,895 → 57,131 | +8.0% | 16.79 → 15.37 |
| 无 WebUI / 2 | 43,693 → 43,690 | -0.0% | 98,072 → 105,710 | +7.8% | 18.07 → 16.71 |
| WebUI / 1 | 23,987 → 24,138 | +0.6% | 49,624 → 52,515 | +5.8% | 18.02 → 16.91 |
| WebUI / 2 | 42,096 → 41,981 | -0.3% | 93,581 → 96,176 | +2.8% | 19.02 → 18.52 |

CPU μs/次由进程 CPU 时间除成功查询数得到；多核可超过 100% CPU。低 QPS 下 CPU 计时粒度较粗，不能过度解读。

| 版本 / CPU | 冷压满 P99 μs 前 → 后 | 热压满 P99 μs 前 → 后 | 100 QPS P99 μs 前 → 后 | 1000 QPS P99 μs 前 → 后 |
|---|---:|---:|---:|---:|
| 无 WebUI / 1 | 2081 → 1656 | 756 → 670 | 248 → 246 | 346 → 332 |
| 无 WebUI / 2 | 1361 → 1358 | 526 → 512 | 269 → 235 | 343 → 372 |
| WebUI / 1 | 1967 → 1714 | 816 → 741 | 239 → 256 | 336 → 340 |
| WebUI / 2 | 1381 → 1354 | 584 → 553 | 268 → 285 | 333 → 337 |

单核冷压满 P99 下降较明显；固定低负载仍受唤醒、调频、采样波动影响，本次没有证据把所有差异归因于其中某一个因素。尤其无 WebUI 双核在 1000 QPS 下 P99 从 343 增到 372 μs，WebUI 低负载也略有退步；保留该方案是以较稳定的热路径 CPU/吞吐收益换取少量受预算约束的内存，不声称达成全面低延迟。

## 内存与实际功能

| 版本 / CPU | 冷阶段末 RSS MiB 前 → 后 | 热阶段末 RSS MiB 前 → 后 |
|---|---:|---:|
| 无 WebUI / 1 | 14.36 → 14.46 | 9.21 → 9.41 |
| 无 WebUI / 2 | 15.19 → 15.12 | 9.09 → 9.71 |
| WebUI / 1 | 15.26 → 15.12 | 9.97 → 10.39 |
| WebUI / 2 | 15.99 → 16.12 | 10.45 → 10.70 |

另做每版 20 秒、400 万域名空间的持续冷查询，双核 WebUI 版每秒采一次 RSS：

- before-webui：852,828 次成功；最大采样 RSS 16.13 MiB，最后采样 16.13 MiB。
- final-webui：848,936 次成功；最大采样 RSS 15.86 MiB，最后采样 15.86 MiB。

该短时检查用于发现随唯一域名持续插入而明显增长的情况，不等同于数天的内存泄漏证明，采样最大值也不等于瞬时峰值。

两个版本、1/2 workers 四组实际路由器检查均通过：IPv4/IPv6 UDP、TCP、域名/监听/客户端 ipset、停止上游后的缓存命中与 ipset 恢复。单 worker WebUI 同时处理 1000 QPS、每 5 秒拉取快照，15,000 次查询全部成功；最终活动查询为 0、历史保留 1000 条、缓存 256 条。

最终源代码：350 项测试通过、0 失败、1 忽略；Windows 全目标 Clippy、AArch64 musl Clippy 和格式检查通过。新增测试核对调用方修改不污染缓存、TTL 递减与过期回复、原始记录保留、预算满时不额外保留。两个 IPK 的可执行载荷逐字节等于实测二进制，权限为 0755。

## C / OxiDNS 参考批次

以下是本轮开始时另一批 63 次对照（7 组合 × 3 场景 × 3 轮），用于判断其他实现的取舍。它使用同一套新增的真正冷查询方法，但与上面的最终批次分开；不要把两批数据拼成严格的 final 对 C/OxiDNS A/B。C 为 Release48.4，OxiDNS 为官方 v1.5.2 full ARM64 musl 产物。

| 实现 / CPU | 冷 QPS | 热 QPS | 冷阶段末 RSS MiB |
|---|---:|---:|---:|
| 本轮前 Rust / 1 | 24,656 | 52,908 | 14.35 |
| 本轮前 Rust / 2 | 43,826 | 97,648 | 15.07 |
| C / 1 | 17,857 | 54,776 | 6.89 |
| C / 2 | 40,538 | 54,580 | 6.85 |
| OxiDNS / 1 | 17,013 | 44,119 | 120.94 |
| OxiDNS / 2 | 29,777 | 80,775 | 211.41 |

本次 OxiDNS 并未更快，冷阶段内存也明显更高。该结果只覆盖给定版本和简单配置；OxiDNS 的缓存容量参数及周期/采样淘汰并非与我们相同的即时上限，不能推导为所有部署的内存表现，也没有与 mosdns 运行对照来验证其 Rust 重写的宣传幅度。

## 为什么这些 Rust 实现能变快

换语言与重构同时发生，不能把全部收益归因于语言。这里区分源码中能直接确认的机制、作者报告的结果，以及在本项目实际测得的收益。

**OxiDNS。** 检查版本为仓库提交 `8183d0fc20192c7d713718ffdde1c6b8b6f3ec91`，包版本 1.5.2；同机对照使用官方 v1.5.2 full ARM64 musl 产物。

- 自有消息和 wire 编解码层直接贯穿执行链；Record 及 RData 采用 Arc 共享，TTL 更新只复制需要修改的部分。[消息模型](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/crates/proto/src/message.rs#L177)、[记录模型](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/crates/proto/src/record.rs)。我们继续使用现有协议库，选择复用已改写 TTL 的响应；没有引入另一套 DNS 解析器。
- 普通小响应能放入 UDP 预算时不做域名压缩，必要时才压缩或截断。这里的收益来自少做工作，同时存在报文长度取舍。[实现](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/crates/proto/src/message.rs#L212)。本轮没有更改我们的报文压缩策略。
- 可复用编码缓冲区，池按需增长、限制保留数量；默认 256 个 8196 字节缓冲区，约 2 MiB 的容量。[缓冲池](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/src/infra/network/buffer_pool.rs)。我们已复用 UDP 上游套接字及其缓冲区，未照搬这个全局池。
- 初始化时解析执行器引用；缓存使用分片表，并降低高占用时的访问时间更新频率，以减少写竞争。[执行链](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/src/plugin/executor/sequence/chain.rs)、[缓存](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/src/plugin/executor/cache/mod.rs#L558)。其采样、周期淘汰与本项目逐次插入后的容量上限不同，不能把 `size: 4096` 直接视为同等内存。
- UDP 仍逐请求创建任务，主服务仍使用 Tokio 多线程运行时；TaskTracker 避免从接收路径反复检查已完成任务。[UDP 入口](https://github.com/svenshi/oxidns/blob/8183d0fc20192c7d713718ffdde1c6b8b6f3ec91/src/plugin/server/udp.rs#L175)。我们试验其对应的非阻塞完成检查后没有稳定收益，已撤回，保留原有 JoinSet 处理。

源码可以解释设计取向，不能单凭这些结构断言某项贡献了多少 QPS。官方产物与我们的构建选项也不完全相同：OxiDNS 仓库 release 默认 opt-level=z、fat LTO，我们使用 opt-level=3、thin LTO；本次比较是可运行产物的比较，不是控制所有编译变量的语言实验。

**Pingora。** Cloudflare 报告其替代原有 NGINX/Lua 服务后，同负载 CPU 降约 70%、内存降约 67%。其解释包含跨线程共享连接池、提高连接复用率和业务逻辑重写；不能理解为把同一份 C 代码翻译成 Rust 就能省去七成 CPU。[作者报告](https://blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/)。后续通过更适合实际查询的数据结构继续节约约 1.28 个百分点的 CPU，也体现了持续按热点优化，而非依赖语言本身。[trie-hard](https://blog.cloudflare.com/pingora-saving-compute-1-percent-at-a-time/)。对 DNS 的对应经验是连接与缓冲区复用、减少重复准备；这些措施需要兼顾连接数和内存上限。

**ripgrep。** 作者将速度归因于有限自动机、字面量预筛、SIMD、批量规则匹配及按负载选择 mmap/缓冲读取。它是 grep 类工具的另一种实现，并非保持所有行为后逐行重写 grep。[项目解释](https://github.com/BurntSushi/ripgrep#is-it-really-faster-than-everything-else)。对应到 DNS，是把固定规则工作移出查询路径、复用结果；不应把面向 x86 的指令优化直接套到所有路由器。

## 对路由器的取舍

优先减少每次查询必做的工作，再检查多线程竞争。单核改善有助于多核，但缓存、原子操作、唤醒和内存带宽会影响扩展性，因此分别测单核与双核。保持已有缓存容量和内存预算，复用的对象也计入预算；没有修改调频策略，没有添加硬件专用指令或删除协议功能。

这台设备提供固定基准，另以 Windows x86-64 测试和 Linux AArch64 构建检查跨平台实现。只有一台 ARM 路由器的性能实测，不能保证其他 SoC 得到相同比例的收益。


## 复现与交付状态

[运行脚本](run_router_efficiency.sh) 需要四核设备、ip-full、iptables、conntrack、taskset，以及由 [router_dnsbench.rs](router_dnsbench.rs) 构建的 `dnsbench`。先创建仅启用 loopback 的专用命名空间，准备脚本和命名为 `smartdns-before`、`smartdns-final`、`smartdns-before-webui`、`smartdns-final-webui` 的产物。脚本会重置该专用空间的 OUTPUT 计数，拒绝在主网络空间运行。

```sh
ip netns add smartdns-efficiency-20260912
ip netns exec smartdns-efficiency-20260912 ip link set lo up
ip netns exec smartdns-efficiency-20260912 env \
  PERF_DIR=/tmp/smartdns-efficiency-20260912 \
  VARIANTS='before-w1 before-w2 final-w1 final-w2 before-webui-w1 before-webui-w2 final-webui-w1 final-webui-w2' \
  SCENARIOS='cold cache latency loaded-latency' RESULTS=final-results.jsonl \
  sh /tmp/smartdns-efficiency-20260912/run_router_efficiency.sh
```

本轮修改暂存供审阅，未提交、未安装到生产服务。测试使用临时文件和隔离命名空间；原有生产程序继续服务。

收尾确认：生产二进制逐字节未变、原 PID 5139 仍在运行；专用命名空间、路由器临时目录和两端临时 HTTP 服务已清理。生产端口 53 的两个域名分别通过 UDP/TCP 共 4 次解析检查。
