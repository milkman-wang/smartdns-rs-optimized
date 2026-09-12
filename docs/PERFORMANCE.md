# 性能测试与优化说明

[返回首页](../README.md)

## 本次比较的是谁

| 名称 | 具体产物 |
|---|---|
| C SmartDNS | pymumu 官方 Release48.4，运行版本 1.2026.08.05-0921，AArch64 |
| 原作者 Rust | mokeyish/smartdns-rs 官方 v0.13.1，AArch64 musl，2026-06-27 构建 |
| 本项目 | smartdns-rs-optimized 0.13.1-24 无界面 ARM64 发布程序，DNS 实现对应 3e4bf27 |

这是各自交付产物的比较，不是控制所有编译选项后的编程语言实验。没有修改原作者程序，也没有把本 fork 的旧版本称为“原版”。

## ARM64：小米 BE10000

QWRT 25.12.2、Linux 5.4.213、Cortex-A73，保持 ondemand 调频及生产配置。测试在独立网络命名空间中运行。服务单核绑定 CPU1，双核绑定 CPU1–2；客户端 CPU0，本地固定上游 CPU3。Rust 分别设 1/2 worker，C 保留自己的线程模型，限制同等 CPU 范围。

缓存容量 4096，热集 256 个域名，冷查询使用 100 万个不循环域名。回复为 192.0.2.1、TTL 3600。三者关闭测速、双栈择优、预取、过期回复、缓存持久化及查询审计。启动后等 1 秒、预热 1 秒；吞吐测试 5 秒、最多 32 个在途请求；低负载为 100 QPS、10 秒、最多 4 个在途请求。每组 3 轮，轮换版本顺序。

### 单核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 18,220 | 54,891 |
| 原作者 Rust v0.13.1 | 5,825 | 5,570 |
| 本项目 0.13.1-24 | 31,015 | 78,679 |

### 双核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 41,232 | 55,371 |
| 原作者 Rust v0.13.1 | 10,508 | 9,228 |
| 本项目 0.13.1-24 | 53,890 | 116,835 |

### 延迟和内存

| 实现 / CPU 核数 | 100 QPS 的 P99，毫秒 | 热查询阶段末 RSS，MiB |
|---|---:|---:|
| C SmartDNS Release48.4 / 1 | 0.175 | 4.75 |
| 原作者 Rust v0.13.1 / 1 | 0.569 | 6.99 |
| 本项目 0.13.1-24 / 1 | 0.179 | 5.80 |
| C SmartDNS Release48.4 / 2 | 0.156 | 4.76 |
| 原作者 Rust v0.13.1 / 2 | 0.512 | 6.55 |
| 本项目 0.13.1-24 / 2 | 0.206 | 5.90 |

各值为三轮中位数，P99 也是逐轮 P99 的中位数。RSS 是小配置阶段末的进程驻留内存，不是完整广告规则部署的占用，也不是进程内存上限。54 次测量共验证 7,233,766 个响应，零错误、零超时。

### 原作者版本的缓存行为

原作者 v0.13.1 的 `DnsCacheMiddleware::handle` 在有效缓存命中后也会创建后台查询，未受 `prefetch-domain no` 控制。因此本轮始终保持模拟上游可用，对三者使用相同条件，并记录实际回源次数；没有把原版额外回源算作工具错误，也没有为了改善其分数改动源码。C 版和本项目的热缓存阶段均不回源。

初次探索采用暂停上游的方法，发现它使原版的后台请求积压，因此整批弃用；首页和本报告只使用重新测量的持续可用上游批次。冷查询的大域名空间避免循环命中；C 版和本项目的回源数均精确等于成功数减去 255 个预热交集。原版命中时也回源，计数按实际值保留。

[原始数据](../contrib/perf/release-comparison-arm64-20260912.json) · [比较运行器](../contrib/perf/run_release_comparison.sh) · [负载工具](../contrib/perf/router_dnsbench.rs)

### 复现

在目标 Linux 设备准备三种程序，命名为 `smartdns-c`、`smartdns-upstream` 和 `smartdns-optimized`。使用 Rust 1.96 编译 `contrib/perf` 的工具并命名为 `dnsbench`，将它们和运行器放到 `/tmp/smartdns-release-comparison`。需要 `ip`、`taskset`、`iptables` 和 `conntrack`，在独立网络命名空间中执行：

```sh
ip netns add smartdns-release-comparison
ip netns exec smartdns-release-comparison ip link set lo up
ip netns exec smartdns-release-comparison env \
  RESULTS=/tmp/smartdns-release-comparison/results.jsonl \
  VARIANTS='c-w1 upstream-w1 optimized-w1 c-w2 upstream-w2 optimized-w2' \
  SCENARIOS='cold cache latency' ROUNDS=3 \
  sh /tmp/smartdns-release-comparison/run_release_comparison.sh
```

结束后保存结果和配置，确认进程已退出，再移除测试命名空间。CPU 分配见上文，不能在未核对 CPU 数量和拓扑时直接用于其他设备。

## x86：Ryzen 7 9800X3D

主机已确认是 Ryzen 7 9800X3D，8 核 16 线程。为让 C 版和两个 Rust 版在相同系统下比较，准备使用 WSL2。Windows 已安装 WSL 组件并启用 VirtualMachinePlatform，但系统要求重启后生效；当前没有 x86 测量结果，不从 ARM 数据推算。

Linux 发行版计划放在 `D:\WSL\Ubuntu`。三个 x86_64 Linux 程序已准备好：C 官方 Release48.4、原作者官方 v0.13.1，以及本 fork 提交 d7d6e11 的 GitHub Actions 无界面构建。后者的 DNS 实现与 ARM 发布版相同，但未使用 ARM 的编译训练数据。完成实测后再加入首页。

## PGO 是什么

PGO 可以理解为“让编译器根据实际运行情况调整程序”：先跑代表性的查询负载，再根据记录重新编译。最终程序不需要一边使用一边采集数据。它不是新 DNS 协议，也不是必须在 LuCI 打开的开关。

本次 ARM64 发布包使用了这项编译优化；普通源码编译不会自动获得全部同等收益。源码中的连接复用、减少复制和缓存响应复用则会保留。不同 CPU、负载与源码版本应重新测量，不能把某一轮提升比例推广到所有设备。

[ARM64 编译优化和有线 LAN 测试](../contrib/perf/wire-pgo-20260912.md) · [实现与内存取舍](../contrib/perf/router-efficiency-20260912.md)
