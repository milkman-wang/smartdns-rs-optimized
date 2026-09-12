# 性能测试与优化说明

[返回首页](../README.md)

## 测试版本与条件

测试日期为 **2026-09-13**。两种平台均重新测量 C SmartDNS、原作者 Rust 和本项目最新主分支，没有沿用旧版本的分数。

| 名称 | 具体程序 |
|---|---|
| C SmartDNS | 官方 Release48.4，运行版本 1.2026.08.05-0921，各平台对应发布程序 |
| 原作者 Rust | mokeyish/smartdns-rs 官方 v0.13.1，各平台对应 musl 发布程序 |
| 本项目 | 主分支提交 657637e236d69a97b2f44f70a381289833f56906，无界面版，包含规则内存修复和平台兼容修复 |

ARM64 程序是本机交叉编译、安装到路由器的 0.13.1-24.4：Rust 1.96.0、openwrt 功能、opt-level 3、thin LTO，沿用 2026-09-12 的 ARM 编译训练数据，没有重新训练。x86 程序来自 [GitHub Actions 构建](https://github.com/milkman-wang/smartdns-rs-optimized/actions/runs/34701487516)的 smartdns-headless-x86_64-unknown-linux-musl-main 附件（artifact 10299639687），为普通 release 构建，没有使用 PGO。

这是实际交付程序的比较；编译选项和依赖也会影响结果，不能将差距全部归因于 Rust 或 C 语言。

两种平台都使用独立网络命名空间和本地固定上游，返回 192.0.2.1、TTL 3600。缓存容量 4096，热集 256 个域名；关闭测速、双栈择优、预取、过期回复、缓存持久化及审计。启动等待 1 秒、预热 1 秒；吞吐测试 5 秒、32 个在途请求；低负载延迟测试 100 QPS、10 秒、4 个在途请求。每组 3 轮，轮换版本顺序。

服务单核绑定 CPU1，双核绑定 CPU1–2；客户端 CPU0，上游 CPU3。Rust 配置 1/2 worker，C 保留自己的线程模型，限制相同 CPU 范围。表内“核数”指允许使用的 CPU 数，不是程序全部线程数。

## ARM64：小米 BE10000

QWRT 25.12.2、Linux 5.4.213、Cortex-A73，保持路由器原有调频和生产服务配置。冷查询使用 100 万个不循环域名，测试进程与生产 DNS 隔离。

### 单核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 18,263 | 56,058 |
| 原作者 Rust v0.13.1 | 5,732 | 5,498 |
| 本项目（2026-09-13 主分支） | 27,878 | 73,892 |

### 双核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 41,939 | 55,138 |
| 原作者 Rust v0.13.1 | 10,356 | 8,807 |
| 本项目（2026-09-13 主分支） | 49,640 | 115,843 |

### 低负载延迟与内存

| 版本 / CPU 核数 | 100 QPS：P50，毫秒 | 100 QPS：P99，毫秒 | 热查询末 RSS，MiB |
|---|---:|---:|---:|
| C SmartDNS Release48.4 / 1 | 0.067 | 0.179 | 4.73 |
| 原作者 Rust v0.13.1 / 1 | 0.292 | 0.565 | 6.96 |
| 本项目（2026-09-13 主分支） / 1 | 0.084 | 0.203 | 8.19 |
| C SmartDNS Release48.4 / 2 | 0.071 | 0.180 | 4.76 |
| 原作者 Rust v0.13.1 / 2 | 0.228 | 0.506 | 6.51 |
| 本项目（2026-09-13 主分支） / 2 | 0.085 | 0.210 | 8.87 |

54 次测量，共验证 **7,036,980 个正确响应**，零错误、零超时。

[原始数据](../contrib/perf/release-comparison-arm64-20260913.json)

与 2026-09-12 的 0.13.1-24 历史记录相比，本轮单核、双核冷查询吞吐分别低约 10.1%、7.9%，热查询分别低约 6.1%、0.8%。这是不同批次的记录对照，本轮没有交替运行旧、新程序，尚不能将差距归因于某项代码修改或编译优化。首页已使用本轮实际结果；新版在本轮同场比较中的吞吐仍高于 C 版和原作者 Rust 版。

## x86：Ryzen 7 9800X3D

主机为 8 核 16 线程 Ryzen 7 9800X3D，WSL 2.7.14、Ubuntu 24.04.4 LTS、Linux 6.18.33.2-microsoft-standard-WSL2。发行版数据盘位于 D:\WSL\Ubuntu，程序和测量文件运行于 Linux 文件系统内。

WSL 报告 16 个逻辑 CPU，CPU0–3 属于不同的核心编号，SMT 同胞为 CPU8–11；本轮按该拓扑选择 CPU0–3。这是 **WSL2 实测**，虚拟 CPU 到物理 CPU 的调度仍由宿主管理，不能当作裸机 Linux 的极限性能，也不能用它直接推算处理器之间的差距。

冷查询域名池扩大到 **一亿个**，避免更快的处理器在 5 秒内重复查询。三方使用同一个已编译的负载工具和相同参数。

### 单核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 21,707 | 112,562 |
| 原作者 Rust v0.13.1 | 12,637 | 31,554 |
| 本项目（2026-09-13 主分支） | 48,614 | 211,253 |

### 双核吞吐

| 版本 | 未缓存查询：次/秒 | 已缓存查询：次/秒 |
|---|---:|---:|
| C SmartDNS Release48.4 | 52,231 | 113,606 |
| 原作者 Rust v0.13.1 | 24,338 | 28,403 |
| 本项目（2026-09-13 主分支） | 98,923 | 400,844 |

### 低负载延迟与内存

| 版本 / CPU 核数 | 100 QPS：P50，毫秒 | 100 QPS：P99，毫秒 | 热查询末 RSS，MiB |
|---|---:|---:|---:|
| C SmartDNS Release48.4 / 1 | 0.127 | 0.219 | 4.91 |
| 原作者 Rust v0.13.1 / 1 | 0.163 | 0.289 | 8.77 |
| 本项目（2026-09-13 主分支） / 1 | 0.135 | 0.232 | 15.92 |
| C SmartDNS Release48.4 / 2 | 0.130 | 0.221 | 4.91 |
| 原作者 Rust v0.13.1 / 2 | 0.242 | 0.415 | 8.47 |
| 本项目（2026-09-13 主分支） / 2 | 0.138 | 0.229 | 20.42 |

54 次测量，共验证 **17,311,590 个正确响应**，零错误、零超时。

[原始数据](../contrib/perf/release-comparison-x86_64-20260913.json)

## 怎样理解这些数据

吞吐、延迟与内存分别衡量不同方面。表中所有值都是三轮中位数，P99 是逐轮 P99 的中位数；原始数据保留每轮结果、CPU 使用率、RSS 和回源次数。低负载延迟包括调度、loopback 协议栈及节能状态，吞吐优势不保证低负载 P99 同样领先。

RSS 是小配置下热查询阶段末的驻留内存，没有加载完整广告和分流名单。大量 Passwall 名单的启动内存及 IPSet 写入延迟另见 [OOM 修复回归报告](../contrib/perf/oom-regression-20260912.md)。负载工具限制 32 个在途请求，因此吞吐表示这组负载下达到的处理量，不是所有并发度下的上限。

原作者 v0.13.1 的缓存命中路径会创建后台刷新，未受 prefetch-domain no 控制。因此每个版本、每种场景均保持模拟上游运行，没有修改原作者程序。C 和本项目的热缓存及低负载阶段回源数均为零；冷查询回源数精确等于成功数减去 255 个预热交集。每个响应都核对查询、返回码和地址，两种平台共 108 次测量，验证 24,348,570 个正确响应，零错误、零超时。

## 复现

准备目标平台的三个程序，分别命名为 smartdns-c、smartdns-upstream、smartdns-optimized。用 Rust 1.96 编译[负载工具](../contrib/perf/router_dnsbench.rs)：

~~~sh
cargo build --release --locked --manifest-path contrib/perf/Cargo.toml
~~~

将生成的 router_dnsbench 复制为 dnsbench，与三个程序及[运行器](../contrib/perf/run_release_comparison.sh)放到 /tmp/smartdns-release-comparison。需要 ip、taskset、iptables 和 conntrack。先用 lscpu -e=CPU,CORE,SOCKET 核对 CPU0–3 的拓扑；以下命令在独立命名空间内执行：

~~~sh
ip netns add smartdns-release-comparison
ip netns exec smartdns-release-comparison ip link set lo up
ip netns exec smartdns-release-comparison env \
  PERF_DIR=/tmp/smartdns-release-comparison \
  RESULTS=/tmp/smartdns-release-comparison/results.jsonl \
  VARIANTS='c-w1 upstream-w1 optimized-w1 c-w2 upstream-w2 optimized-w2' \
  SCENARIOS='cold cache latency' COLD_DOMAINS=100000000 ROUNDS=3 \
  sh /tmp/smartdns-release-comparison/run_release_comparison.sh
ip netns del smartdns-release-comparison
~~~

x86 本轮使用 COLD_DOMAINS=100000000，ARM 本轮使用默认值 1000000。结果文件每行是一条 JSON，包含轮次、版本、worker 数、回源数和指标；保存后按平台、版本、worker 数和场景分别取中位数。

## PGO 是什么

PGO 可以理解为“让编译器根据实际运行情况调整程序”：先跑代表性查询，再根据记录重新编译。最终程序无需在使用时采集数据，也没有需要在 LuCI 打开的开关。

本轮 ARM 使用这种编译优化，x86 没有使用。源码中的连接复用、减少复制和缓存响应复用都会保留；不同平台的训练结果不能直接套用。

[ARM 编译优化与有线 LAN 测试](../contrib/perf/wire-pgo-20260912.md) · [实现与内存取舍](../contrib/perf/router-efficiency-20260912.md) · [2026-09-12 ARM 历史数据](../contrib/perf/release-comparison-arm64-20260912.json)
